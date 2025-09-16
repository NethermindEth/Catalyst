import time

def send_transaction(nonce : int, account, amount, eth_client, private_key):
    base_fee = eth_client.eth.get_block('latest')['baseFeePerGas']
    if base_fee < 25000000:
        base_fee = 25000000
    priority_fee = eth_client.eth.max_priority_fee
    max_fee_per_gas = base_fee * 2 + priority_fee
    tx = {
        'nonce': nonce,
        'to': '0x0000000000000000000000000000000000000001',
        'value': eth_client.to_wei(amount, 'ether'),
        'gas': 40000,
        'maxFeePerGas': max_fee_per_gas,
        'maxPriorityFeePerGas': priority_fee,
        'chainId': eth_client.eth.chain_id,
        'type': 2  # EIP-1559 transaction type
    }

    print(f'RPC URL: {eth_client.provider.endpoint_uri}, Sending from: {account.address}')
    signed_tx = eth_client.eth.account.sign_transaction(tx, private_key)
    tx_hash = eth_client.eth.send_raw_transaction(signed_tx.raw_transaction)
    print(f'Transaction sent: {tx_hash.hex()}')
    return tx_hash

def wait_for_secs(seconds):
    for i in range(seconds, 0, -1):
        print(f'Waiting for {i:02d} seconds', end='\r')
        time.sleep(1)
    print('')

def get_slot_in_epoch(beacon_client):
    slots_per_epoch = int(beacon_client.get_spec()['data']['SLOTS_PER_EPOCH'])
    current_slot = int(beacon_client.get_syncing()['data']['head_slot'])
    return current_slot % slots_per_epoch

def get_seconds_to_handover_window(beacon_client):
    slot_in_epoch = get_slot_in_epoch(beacon_client)
    if slot_in_epoch < 28:
        return (28 - slot_in_epoch) * int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])
    else:
        return 0

def wait_for_tx_to_be_included(eth_client, tx_hash):
    try:
        eth_client.eth.wait_for_transaction_receipt(tx_hash, timeout=10)
        return True
    except Exception as e:
        print(f"Error waiting for transaction to be included: {e}")
        return False

def wait_for_new_block(eth_client, initial_block_number):
    for i in range(10):
        if eth_client.eth.block_number > initial_block_number:
            return True
        time.sleep(1)
    print(f"Error waited 10 seconds for new block, but block number did not increase")
    return False

def wait_for_handover_window(beacon_client):
    seconds_to_handover_window = get_seconds_to_handover_window(beacon_client)
    print(f"Seconds to handover window: {seconds_to_handover_window}")
    wait_for_secs(seconds_to_handover_window)