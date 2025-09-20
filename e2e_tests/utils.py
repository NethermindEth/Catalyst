import time
import web3
import subprocess
import json
import os

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

    print(f'RPC URL: {eth_client.provider.endpoint_uri}, Sending from: {account.address}, nonce: {nonce}')
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
        receipt = eth_client.eth.wait_for_transaction_receipt(tx_hash, timeout=10)
        if receipt.status == 1:
            return True
        else:
            print(f"Transaction {tx_hash} reverted")
            return False
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

def wait_for_slot_beginning(beacon_client, desired_slot):
    slot_in_epoch = get_slot_in_epoch(beacon_client)
    seconds_per_slot = int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])
    print(f"Slot in epoch: {slot_in_epoch}")
    number_of_slots_in_epoch = int(beacon_client.get_spec()['data']['SLOTS_PER_EPOCH'])

    slots_to_wait = (number_of_slots_in_epoch - slot_in_epoch + desired_slot) % number_of_slots_in_epoch - 1
    if slots_to_wait < 0:   # if we are in the desired slot, we need to wait for the next epoch
        slots_to_wait = number_of_slots_in_epoch - 1
    seconds_till_end_of_slot = seconds_per_slot - int(time.time()) % seconds_per_slot

    seconds_to_wait = seconds_till_end_of_slot + slots_to_wait * seconds_per_slot + 1  # +1 second to be sure we are in the next slot
    print(f"Seconds to wait: {seconds_to_wait}")

    wait_for_secs(seconds_to_wait)

def spam_n_txs(eth_client, private_key, n):
    account = eth_client.eth.account.from_key(private_key)
    last_tx_hash = None
    for i in range(n):
        nonce = eth_client.eth.get_transaction_count(account.address)
        last_tx_hash = send_transaction(nonce, account, '0.00009', eth_client, private_key)
        wait_for_tx_to_be_included(eth_client, last_tx_hash)
    return last_tx_hash

def spam_n_blocks(eth_client, private_key, n, preconf_min_txs):
    """Spam as many tx to create n blocks, wait for each block to be mined"""
    account = eth_client.eth.account.from_key(private_key)
    last_tx_hash = None
    for i in range(n):
        nonce = eth_client.eth.get_transaction_count(account.address)
        for j in range(preconf_min_txs):
            last_tx_hash = send_transaction(nonce, account, '0.00009', eth_client, private_key)
            nonce += 1
        wait_for_tx_to_be_included(eth_client, last_tx_hash)
    return last_tx_hash

def wait_for_batch_proposed_event(eth_client, taiko_inbox_address, from_block):
    with open("../whitelist/src/l1/abi/ITaikoInbox.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=taiko_inbox_address, abi=abi)

    # Create an event filter for BatchProposed events
    batch_proposed_filter = contract.events.BatchProposed.create_filter(
        from_block=from_block
    )

    wait_time = 0;
    while True:
        if wait_time > 100:
            assert False, "Warning waited 100 seconds for BatchProposed event without getting one"

        new_entries = batch_proposed_filter.get_all_entries()
        if len(new_entries) > 0:
            event = new_entries[-1]
            print_batch_info(event)
            return event

        time.sleep(1)
        wait_time += 1

def print_batch_info(event):
    print("BatchProposed event detected:")
    print(f"  Batch ID: {event['args']['meta']['batchId']}")
    print(f"  Proposer: {event['args']['meta']['proposer']}")
    print(f"  Proposed At: {event['args']['meta']['proposedAt']}")
    print(f"  Last Block ID: {event['args']['info']['lastBlockId']}")
    print(f"  Last Block Timestamp: {event['args']['info']['lastBlockTimestamp']}")
    print(f"  Transaction Hash: {event['transactionHash'].hex}")
    print(f"  Block Number: {event['blockNumber']}")
    print("---")

def get_current_operator(eth_client, l1_contract_address):
    with open("../whitelist/src/l1/abi/PreconfWhitelist.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=l1_contract_address, abi=abi)
    return contract.functions.getOperatorForCurrentEpoch().call()

def get_next_operator(eth_client, l1_contract_address):
    import json
    with open("../whitelist/src/l1/abi/PreconfWhitelist.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=l1_contract_address, abi=abi)
    return contract.functions.getOperatorForNextEpoch().call()

def spam_txs_until_new_batch_is_proposed(l1_eth_client, l2_eth_client, private_key, taiko_inbox_address, beacon_client, preconf_min_txs):
    current_block = l1_eth_client.eth.block_number
    l1_slot_duration = int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])

    number_of_blocks = 10
    for i in range(number_of_blocks):
        spam_n_blocks(l2_eth_client, private_key, 1, preconf_min_txs)
        wait_till_next_l1_slot(beacon_client)
        event = get_last_batch_proposed_event(l1_eth_client, taiko_inbox_address, current_block)
        if event is not None:
            return event

    wait_for_batch_proposed_event(l1_eth_client, taiko_inbox_address, current_block)

def wait_till_next_l1_slot(beacon_client):
    l1_slot_duration = int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])
    current_time = int(time.time()) % l1_slot_duration
    time.sleep(l1_slot_duration - current_time)

def get_last_batch_proposed_event(eth_client, taiko_inbox_address, from_block):
    with open("../whitelist/src/l1/abi/ITaikoInbox.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=taiko_inbox_address, abi=abi)
    batch_proposed_filter = contract.events.BatchProposed.create_filter(
        from_block=from_block
    )
    new_entries = batch_proposed_filter.get_all_entries()
    if len(new_entries) > 0:
        event = new_entries[-1]
        print_batch_info(event)
        return event
    return None

def stop_catalyst_node(node_number):
    container_name = choose_catalyst_node(node_number)

    result = subprocess.run(["docker", "stop", container_name], capture_output=True, text=True, check=True)
    print(result.stdout)
    if result.stderr:
        print(result.stderr)

def start_catalyst_node(node_number):
    container_name = choose_catalyst_node(node_number)

    result = subprocess.run(["docker", "start", container_name], capture_output=True, text=True, check=True)
    print(result.stdout)
    if result.stderr:
        print(result.stderr)

def choose_catalyst_node(node_number):
    container_name = "catalyst-node-1" if node_number == 1 else "catalyst-node-2" if node_number == 2 else None
    if container_name is None:
        raise Exception("Invalid node number")
    return container_name

def is_catalyst_node_running(node_number):
    container_name = choose_catalyst_node(node_number)
    try:
        result = subprocess.run(
            ["docker", "inspect", "-f", "{{.State.Running}}", container_name],
            capture_output=True,
            text=True,
            check=True
        )
        return result.stdout.strip() == "true"
    except subprocess.CalledProcessError:
        return False

def ensure_catalyst_node_running(node_number):
    """Ensure the catalyst node is running, start it if it's not"""
    if not is_catalyst_node_running(node_number):
        print(f"Catalyst node {node_number} is not running, starting it...")
        start_catalyst_node(node_number)
    else:
        print(f"Catalyst node {node_number} is already running")

def sleep_until_slot_in_epoch(beacon_client, slot):
    sec = get_seconds_to_slot_in_epoch(beacon_client, slot)
    print("Sleep for", sec, "s")
    time.sleep(sec)

def get_seconds_to_slot_in_epoch(beacon_client, slot):
    spec = beacon_client.get_spec()
    sec_per_slot = int(spec['data']['SECONDS_PER_SLOT'])
    slots_per_epoch = int(spec['data']['SLOTS_PER_EPOCH'])
    current_slot = int(beacon_client.get_syncing()['data']['head_slot'])
    slot_in_epoch = current_slot % slots_per_epoch
    if slot_in_epoch == slot:
        return 0
    elif slot_in_epoch < slot:
        return (slot - slot_in_epoch) * sec_per_slot
    else:  # slot_in_epoch > slot
        return (slots_per_epoch - (slot_in_epoch - slot)) * sec_per_slot

def spam_n_txs_no_wait(eth_client, private_key, n, delay):
    account = eth_client.eth.account.from_key(private_key)
    last_tx_hash = None
    nonce = eth_client.eth.get_transaction_count(account.address)
    for i in range(n):
        last_tx_hash = send_transaction(nonce+i, account, '0.00009', eth_client, private_key)
        time.sleep(delay)
    wait_for_tx_to_be_included(eth_client, last_tx_hash)

def get_slot_duration_sec(beacon_client):
    return int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])
