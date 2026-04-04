const { Connection, Keypair, VersionedTransaction, PublicKey, Transaction } = require('@solana/web3.js');
const { createJupiterApiClient } = require('@jup-ag/api');
const { 
    getAssociatedTokenAddress, 
    createTransferInstruction, 
    getAccount, 
    createAssociatedTokenAccountInstruction,
    TOKEN_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID
} = require('@solana/spl-token');
const bs58 = require('bs58').default;

async function getTokenProgramId(connection, mintPubkey) {
    const mintInfo = await connection.getAccountInfo(mintPubkey);
    if (!mintInfo) {
        throw new Error(`Mint account not found: ${mintPubkey.toBase58()}`);
    }
    
    const ownerStr = mintInfo.owner.toBase58();
    
    if (ownerStr === TOKEN_2022_PROGRAM_ID.toBase58()) {
        console.error(`Token type: Token-2022`);
        return TOKEN_2022_PROGRAM_ID;
    } else if (ownerStr === TOKEN_PROGRAM_ID.toBase58()) {
        console.error(`Token type: SPL Token (standard)`);
        return TOKEN_PROGRAM_ID;
    } else {
        throw new Error(`Unknown token program: ${ownerStr}`);
    }
}

async function main() {
    const args = process.argv.slice(2);
    if (args.length < 4) {
        console.error('Usage: node swap.js <privkey_bs58> <amount_lamports> <output_mint> <destination_wallet>');
        process.exit(1);
    }

    const [privkeyBs58, amountLamports, outputMint, destinationWallet] = args;

    const rpcUrl = process.env.SOLANA_RPC_URL || 'https://api.mainnet-beta.solana.com';
    const connection = new Connection(rpcUrl, 'confirmed');
    const keypair = Keypair.fromSecretKey(bs58.decode(privkeyBs58));
    const jupiter = createJupiterApiClient();
    const destPubkey = new PublicKey(destinationWallet);
    const mintPubkey = new PublicKey(outputMint);

    console.error(`Swapping ${amountLamports} lamports to ${outputMint}`);
    console.error(`Signer: ${keypair.publicKey.toBase58()}`);
    console.error(`Destination: ${destinationWallet}`);

    try {
        // Detect token program (SPL Token vs Token-2022)
        const tokenProgramId = await getTokenProgramId(connection, mintPubkey);

        // Step 1: Get quote
        const quote = await jupiter.quoteGet({
            inputMint: 'So11111111111111111111111111111111111111112',
            outputMint: outputMint,
            amount: parseInt(amountLamports),
            slippageBps: 1000,  // 10% slippage for volatile tokens
        });

        if (!quote) {
            throw new Error('No quote available');
        }
        console.error(`Quote: ${quote.outAmount} tokens`);

        // Step 2: Get swap transaction
        const swapResult = await jupiter.swapPost({
            swapRequest: {
                userPublicKey: keypair.publicKey.toBase58(),
                quoteResponse: quote,
                wrapAndUnwrapSol: true,
                autoSlippage: true,
                autoSlippageCollisionUsdValue: 1000,
                dynamicComputeUnitLimit: true,
                prioritizationFeeLamports: {
                    priorityLevelWithMaxLamports: {
                        priorityLevel: 'medium',
                        maxLamports: 500000,
                    },
                },
            },
        });

        // Step 3: Deserialize and sign transaction
        const swapTxBuf = Buffer.from(swapResult.swapTransaction, 'base64');
        const transaction = VersionedTransaction.deserialize(swapTxBuf);
        transaction.sign([keypair]);

        // Step 4: Send swap transaction
        const swapSig = await connection.sendRawTransaction(transaction.serialize(), {
            skipPreflight: true,
            maxRetries: 3,
        });
        console.error(`Swap TX sent: ${swapSig}`);

        // Step 5: Confirm swap
        const confirmation = await connection.confirmTransaction(swapSig, 'confirmed');
        if (confirmation.value.err) {
            throw new Error(`Swap failed: ${JSON.stringify(confirmation.value.err)}`);
        }
        console.error('Swap confirmed!');

        // Step 6: Get token balance and transfer to destination
        // Use correct program ID for ATA derivation
        const signerAta = await getAssociatedTokenAddress(
            mintPubkey, 
            keypair.publicKey,
            false,
            tokenProgramId,
            ASSOCIATED_TOKEN_PROGRAM_ID
        );
        const destAta = await getAssociatedTokenAddress(
            mintPubkey, 
            destPubkey,
            false,
            tokenProgramId,
            ASSOCIATED_TOKEN_PROGRAM_ID
        );

        // Wait for balance to update and retry
        let tokenBalance = 0n;
        for (let i = 0; i < 5; i++) {
            await new Promise(r => setTimeout(r, 2000));
            try {
                const tokenAccount = await getAccount(connection, signerAta, 'confirmed', tokenProgramId);
                tokenBalance = tokenAccount.amount;
                console.error(`Token balance: ${tokenBalance}`);
                break;
            } catch (e) {
                console.error(`Waiting for token account... attempt ${i + 1}`);
                if (i === 4) throw new Error(`Token account not found after swap`);
            }
        }

        if (tokenBalance > 0n) {
            const transferTx = new Transaction();

            // Check if dest ATA exists, create if not
            let destAtaExists = false;
            try {
                await getAccount(connection, destAta, 'confirmed', tokenProgramId);
                destAtaExists = true;
            } catch (e) {
                destAtaExists = false;
            }

            if (!destAtaExists) {
                console.error('Creating destination ATA...');
                transferTx.add(
                    createAssociatedTokenAccountInstruction(
                        keypair.publicKey,
                        destAta,
                        destPubkey,
                        mintPubkey,
                        tokenProgramId,
                        ASSOCIATED_TOKEN_PROGRAM_ID
                    )
                );
            }

            // Transfer all tokens to destination
            transferTx.add(
                createTransferInstruction(
                    signerAta,
                    destAta,
                    keypair.publicKey,
                    tokenBalance,
                    [],
                    tokenProgramId
                )
            );

            transferTx.feePayer = keypair.publicKey;
            transferTx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;
            transferTx.sign(keypair);

            const transferSig = await connection.sendRawTransaction(transferTx.serialize());
            
            // Confirm with longer timeout and retry
            let confirmed = false;
            for (let attempt = 0; attempt < 3; attempt++) {
                try {
                    const result = await connection.confirmTransaction({
                        signature: transferSig,
                        blockhash: transferTx.recentBlockhash,
                        lastValidBlockHeight: (await connection.getLatestBlockhash()).lastValidBlockHeight
                    }, 'confirmed');
                    if (!result.value.err) {
                        confirmed = true;
                        break;
                    }
                } catch (e) {
                    console.error(`Confirm attempt ${attempt + 1} failed: ${e.message}`);
                    if (attempt < 2) {
                        await new Promise(r => setTimeout(r, 5000)); // Wait 5s before retry
                    }
                }
            }
            
            if (!confirmed) {
                // Check if transaction actually succeeded
                const status = await connection.getSignatureStatus(transferSig);
                if (status.value && status.value.confirmationStatus === 'confirmed') {
                    confirmed = true;
                }
            }
            
            if (confirmed) {
                console.error(`Token transfer confirmed: ${transferSig}`);
            } else {
                console.error(`Token transfer sent but unconfirmed: ${transferSig}`);
            }
        }

        // Output final signature
        console.log(swapSig);

    } catch (error) {
        console.error(`Error: ${error.message || error}`);
        process.exit(1);
    }
}

main();
