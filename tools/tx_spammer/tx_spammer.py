import time
from web3 import Web3
import os
from dotenv import load_dotenv
import argparse
import requests
import json
import asyncio
from concurrent.futures import ThreadPoolExecutor

# Load environment variables from .env file
load_dotenv()

def get_beacon_genesis_timestamp(beacon_rpc_url):
    try:
        response = requests.get(f"{beacon_rpc_url}/eth/v1/beacon/genesis")
        response.raise_for_status()
        genesis_data = json.loads(response.text)
        return int(genesis_data['data']['genesis_time'])
    except requests.RequestException as e:
        print(f"Error fetching beacon genesis timestamp: {e}")
        return None

private_key = os.getenv('PRIVATE_KEY')
if not private_key:
    raise Exception("Environment variable PRIVATE_KEY not set")

recipient = os.getenv('RECIPIENT_ADDRESS')
if not recipient:
    raise Exception("Environment variable RECIPIENT_ADDRESS not set")

parser = argparse.ArgumentParser(description='Spam transactions on the Taiko network.')
parser.add_argument('--count', type=int, default=1, help='Number of transactions to send')
parser.add_argument('--amount', type=float, default=0.006, help='Amount of ETH to send per transaction')
parser.add_argument('--rpc', type=str, default='https://RPC.helder.taiko.xyz', help='RPC URL for the Taiko network')
parser.add_argument('--slots', nargs='+', type=int, default=[],
                    help='Slots to send transactions (0-31)')
parser.add_argument('--beacon-rpc', type=str, help='Beacon RPC URL for the Taiko network')
parser.add_argument('--sleep', type=float, default=2.0, help='Sleep time between transactions in seconds')
parser.add_argument('--batch-size', type=int, default=100, help='Number of transactions to send in parallel')
args = parser.parse_args()


genesis_timestamp = None
if len(args.slots) > 0:
    if args.beacon_rpc is None:
        raise Exception("Beacon RPC URL is required when specifying slots")
    print(f'Sending transactions for slots: {args.slots}')
    genesis_timestamp = get_beacon_genesis_timestamp(args.beacon_rpc)
    if genesis_timestamp is None:
        raise Exception("Failed to get beacon genesis timestamp")
    else:
        print(f'Beacon genesis timestamp: {genesis_timestamp}')

# Connect to the Taiko network
w3 = Web3(Web3.HTTPProvider(args.rpc))

# Check if connected
if not w3.is_connected():
    raise Exception("Failed to connect to the Taiko network")

# Get the account from the private key
account = w3.eth.account.from_key(private_key)
amount = w3.to_wei(args.amount, 'ether')
print(f'Sending transactions from: {account.address}')

def spam_transactions(count):
    # Create batches of transactions
    batch_size = min(args.batch_size, count)
    print(f"Sending {count} transactions in batches of {batch_size}")

    chain_id = w3.eth.chain_id

    # Get gas parameters once per batch to reduce RPC calls
    try:
        estimated_gas = w3.eth.estimate_gas({
            'to': recipient,
            'value': amount,
            'from': account.address
        })
        gas_limit = int(estimated_gas * 1.2)  # Add 20% buffer to avoid out-of-gas errors
    except Exception as e:
        print(f"Gas estimation failed: {e}")
        gas_limit = 40000

    sent_count = 0
    while sent_count < count:
        # Get latest gas parameters for this batch
        base_fee = w3.eth.get_block('latest')['baseFeePerGas']
        if base_fee < 25000000:
            base_fee = 25000000
        priority_fee = w3.eth.max_priority_fee
        max_fee_per_gas = base_fee * 2 + priority_fee

        # Sign all transactions in this batch at once
        batch_count = min(batch_size, count - sent_count)
        signed_txs = []

        pending_nonce = w3.eth.get_transaction_count(account.address, 'pending')

        for i in range(batch_count):
            nonce = pending_nonce + i
            tx = {
                'nonce': nonce,
                'to': recipient,
                'value': amount,
                'gas': gas_limit,
                'maxFeePerGas': max_fee_per_gas,
                'maxPriorityFeePerGas': priority_fee,
                'chainId': chain_id,
                'type': 2  # EIP-1559 transaction type
            }
            signed_tx = w3.eth.account.sign_transaction(tx, private_key)
            signed_txs.append(signed_tx)

        # Send all signed transactions in parallel
        with ThreadPoolExecutor(max_workers=batch_count) as executor:
            def send_raw_tx(signed_tx):
                try:
                    tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
                    return tx_hash.hex()
                except Exception as e:
                    print(f"Error sending transaction: {e}")
                    return None

            futures = [executor.submit(send_raw_tx, signed_tx) for signed_tx in signed_txs]

            for i, future in enumerate(futures):
                try:
                    future.result()
                except Exception as e:
                    print(f"Error processing transaction result: {e}")

        sent_count += batch_count
        print(f"Sent {sent_count}/{count} transactions starting from nonce {pending_nonce}")

        # Sleep between batches if there are more to send
        if sent_count < count:
            time.sleep(args.sleep)


if len(args.slots) > 0:
    for slot in args.slots:
        current_time = int(time.time())
        current_slot = (current_time - genesis_timestamp) // 12  # Assuming 12-second slot time
        current_epoch_slot = current_slot % 32
        print(f'Current slot: {current_slot}, current_epoch_slot: {current_epoch_slot}')

        # Calculate the time until the next occurrence of the desired slot
        time_since_epoch_start = (current_time - genesis_timestamp) % (32 * 12)
        time_until_slot = ((slot - current_epoch_slot) % 32) * 12 - (time_since_epoch_start % 12)

        if time_until_slot <= 0:
            time_until_slot += 32 * 12  # Wait for the next epoch if we've missed the slot in this epoch

        print(f'Waiting {time_until_slot} seconds for slot {slot}')
        time.sleep(time_until_slot)

        print(f'Spamming transactions for slot {slot}')
        spam_transactions(args.count)
else:
    spam_transactions(args.count)