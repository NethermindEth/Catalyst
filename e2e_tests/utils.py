import time
import web3
import subprocess
import json
import os
import requests
import re
from forced_inclusion_store import pacaya_fi_abi

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

    # Get current UTC time with microseconds
    now = time.gmtime()
    current_time = time.strftime("%H:%M:%S", now) + f".{int(time.time()*1e6)%1000000:06d}Z"

    print(f'RPC URL: {eth_client.provider.endpoint_uri}, Sending from: {account.address}, nonce: {nonce}, time: {current_time}')
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

def wait_for_tx_to_be_included(eth_client, tx_hash, timeout=10):
    try:
        receipt = eth_client.eth.wait_for_transaction_receipt(tx_hash, timeout=timeout)
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

def wait_for_next_slot(beacon_client):
    slot_in_epoch = get_slot_in_epoch(beacon_client)
    wait_for_slot_beginning(beacon_client,slot_in_epoch+1)

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
    print(f"Spamming {n} blocks with {preconf_min_txs} transactions per block")
    account = eth_client.eth.account.from_key(private_key)
    last_tx_hash = None
    for i in range(n):
        nonce = eth_client.eth.get_transaction_count(account.address)
        for j in range(preconf_min_txs):
            last_tx_hash = send_transaction(nonce, account, '0.00009', eth_client, private_key)
            nonce += 1
        wait_for_tx_to_be_included(eth_client, last_tx_hash)
    return last_tx_hash

def spam_n_txs_wait_only_for_the_last(eth_client, private_key, n, delay):
    account = eth_client.eth.account.from_key(private_key)
    last_tx_hash = None
    nonce = eth_client.eth.get_transaction_count(account.address)
    for i in range(n):
        last_tx_hash = send_transaction(nonce+i, account, '0.00009', eth_client, private_key)
        time.sleep(delay)
    wait_for_tx_to_be_included(eth_client, last_tx_hash)

def send_n_txs_without_waiting(eth_client, private_key, n):
    account = eth_client.eth.account.from_key(private_key)
    nonce = eth_client.eth.get_transaction_count(account.address)
    for i in range(n):
        send_transaction(nonce+i, account, '0.00009', eth_client, private_key)

def wait_for_batch_proposed_event(eth_client, from_block, env_vars):
    print(f"Waiting for BatchProposed event from block {from_block}")
    proposed_filter = get_proposed_event_filter(eth_client, from_block, env_vars)

    WAIT_TIME = 100
    for i in range(WAIT_TIME):
        new_entries = proposed_filter.get_all_entries()
        if len(new_entries) > 0:
            print(f"Got BatchProposed event after {i} seconds")
            event = new_entries[-1]
            print_batch_info(eth_client, event, env_vars)
            return event
        time.sleep(1)
    assert False, "Warning waited {} seconds for BatchProposed event without getting one".format(WAIT_TIME)

def get_proposed_event_filter(eth_client, from_block, env_vars):
    if env_vars.is_pacaya():
        with open("../pacaya/src/l1/abi/ITaikoInbox.json") as f:
            abi = json.load(f)
        contract = eth_client.eth.contract(address=env_vars.taiko_inbox_address, abi=abi)
        return contract.events.BatchProposed.create_filter(
            from_block=from_block
        )
    elif env_vars.is_shasta():
        proposed_event_abi = get_shasta_inbox_abi()
        contract = eth_client.eth.contract(address=env_vars.taiko_inbox_address, abi=proposed_event_abi)
        return contract.events.Proposed.create_filter(
            from_block=from_block
        )
    else:
        raise Exception("Invalid protocol")

def wait_for_forced_inclusion_store_to_be_empty(l1_client, env_vars):
    TIMEOUT = 300
    i = 0
    while not forced_inclusion_store_is_empty(l1_client, env_vars):
        if i >= TIMEOUT:
            assert False, "Error: waited {} seconds for forced inclusion store to be empty".format(TIMEOUT)
        time.sleep(1)
        i += 1

def print_batch_info(l1_client, event, env_vars):
    print("BatchProposed event detected:")
    if env_vars.is_pacaya():
        print(f"  Batch ID: {event['args']['meta']['batchId']}")
        print(f"  Proposer: {event['args']['meta']['proposer']}")
        print(f"  Proposed At: {event['args']['meta']['proposedAt']}")
        print(f"  Last Block ID: {event['args']['info']['lastBlockId']}")
        print(f"  Last Block Timestamp: {event['args']['info']['lastBlockTimestamp']}")
        print(f"  Transaction Hash: {event['transactionHash'].hex()}")
        print(f"  Block Number: {event['blockNumber']}")
    else:
        print(f"  Proposal ID: {event['args']['id']}")
        print(f"  Proposer: {event['args']['proposer']}")
        print(f"  End of submission window timestamp: {event['args']['endOfSubmissionWindowTimestamp']}")
        print(f"  Transaction Hash: {event['transactionHash'].hex()}")
        print(f"  Block number: {event['blockNumber']}")
    print("---")

def get_current_operator(eth_client, l1_contract_address):
    with open("../pacaya/src/l1/abi/PreconfWhitelist.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=l1_contract_address, abi=abi)
    return contract.functions.getOperatorForCurrentEpoch().call()

def get_next_operator(eth_client, l1_contract_address):
    import json
    with open("../pacaya/src/l1/abi/PreconfWhitelist.json") as f:
        abi = json.load(f)

    contract = eth_client.eth.contract(address=l1_contract_address, abi=abi)
    return contract.functions.getOperatorForNextEpoch().call()

def spam_txs_until_new_batch_is_proposed(l1_eth_client, l2_eth_client, beacon_client, env_vars):
    current_block = l1_eth_client.eth.block_number
    l1_slot_duration = int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])

    number_of_blocks = 10
    for i in range(number_of_blocks):
        spam_n_blocks(l2_eth_client, env_vars.l2_prefunded_priv_key, 1, env_vars.preconf_min_txs)
        wait_till_next_l1_slot(beacon_client)
        event = get_last_batch_proposed_event(l1_eth_client, current_block, env_vars)
        if event is not None:
            return event

    wait_for_batch_proposed_event(l1_eth_client, current_block, env_vars)

def wait_till_next_l1_slot(beacon_client):
    l1_slot_duration = int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])
    current_time = int(time.time()) % l1_slot_duration
    time.sleep(l1_slot_duration - current_time)

def get_last_batch_proposed_event(eth_client, from_block, env_vars):
    proposed_filter = get_proposed_event_filter(eth_client, from_block, env_vars)
    new_entries = proposed_filter.get_all_entries()
    if len(new_entries) > 0:
        event = new_entries[-1]
        print_batch_info(eth_client, event, env_vars)
        return event
    return None

def stop_catalyst_node(node_number):
    container_name = choose_catalyst_node(node_number)

    result = subprocess.run(["docker", "stop", container_name], capture_output=True, text=True, check=True)
    print(f"Stop {result.stdout}")
    if result.stderr:
        print(result.stderr)

def start_catalyst_node(node_number):
    container_name = choose_catalyst_node(node_number)

    result = subprocess.run(["docker", "start", container_name], capture_output=True, text=True, check=True)
    print(f"Start {result.stdout}")
    if result.stderr:
        print(result.stderr)

def restart_catalyst_node(node_number):
    container_name = choose_catalyst_node(node_number)

    result = subprocess.run(["docker", "restart", container_name], capture_output=True, text=True, check=True)
    print(f"Restart {result.stdout}")
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

def get_current_operator_number(l1_client, l2_prefunded_priv_key, preconf_whitelist_address):
    account1 = l1_client.eth.account.from_key(l2_prefunded_priv_key)
    current_operator = get_current_operator(l1_client, preconf_whitelist_address)
    return 1 if current_operator == account1.address else 2

def get_slot_duration_sec(beacon_client):
    return int(beacon_client.get_spec()['data']['SECONDS_PER_SLOT'])

def get_two_l2_slots_duration_sec(preconf_heartbeat_ms):
     return int(preconf_heartbeat_ms / 500) # preconf_heartbeat_ms / 1000 * 2

def wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, preconf_whitelist_address, desired_slot):
    """Wait for the epoch after which the operator will switch and given slot"""
    for i in range(100):
        ## start early to be sure we finish current batch and add single block to the next batch
        wait_for_slot_beginning(beacon_client, desired_slot)
        current_operator = get_current_operator(l1_client, preconf_whitelist_address)
        next_operator = get_next_operator(l1_client, preconf_whitelist_address)
        print(f"Current operator: {current_operator}")
        print(f"Next operator: {next_operator}")
        if current_operator != next_operator:
            break
    assert current_operator != next_operator, "Current operator should be different from next operator"

def read_shasta_inbox_config(l1_client, shasta_inbox_address):
    abi = get_shasta_inbox_abi()
    contract = l1_client.eth.contract(address=shasta_inbox_address, abi=abi)
    config = contract.functions.getConfig().call()
    return config

def get_shasta_inbox_abi():
    commit = get_taiko_bindings_commit()
    url = f"https://raw.githubusercontent.com/taikoxyz/taiko-mono/{commit}/packages/taiko-client-rs/crates/bindings/src/inbox.rs"
    return read_json_abi_from_rust_bindings(url)

def get_taiko_bindings_commit():
    """Read the commit hash from Cargo.toml for taiko_bindings dependency"""
    cargo_toml_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), "Cargo.toml")
    with open(cargo_toml_path, 'r') as f:
        content = f.read()

    # Find the taiko_bindings dependency and extract the rev value
    pattern = r'taiko_bindings\s*=\s*\{[^}]*rev\s*=\s*"([^"]+)"'
    match = re.search(pattern, content)

    if not match:
        raise ValueError("Could not find taiko_bindings rev in Cargo.toml")

    return match.group(1)

def read_json_abi_from_rust_bindings(url):
    response = requests.get(url)
    response.raise_for_status()  # Raise an exception for bad status codes

    content = response.text

    # Find the ```json code block
    pattern = r'```json\s*\n(.*?)\n```'
    match = re.search(pattern, content, re.DOTALL)

    if not match:
        raise ValueError(f"Could not find ```json code block in the file at {url}")

    json_content = match.group(1).strip()

    # Parse and return the JSON
    return json.loads(json_content)

def get_forced_inclusion_store_head(l1_client, env_vars):
    if env_vars.is_pacaya():
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=pacaya_fi_abi)
        head = contract.functions.head().call()
        return int(head)
    else:
        shasta_abi = get_shasta_inbox_abi()
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=shasta_abi)
        head, tail = contract.functions.getForcedInclusionState().call()
        return int(head)

def forced_inclusion_store_is_empty(l1_client, env_vars):
    if env_vars.is_pacaya():
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=pacaya_fi_abi)
        head = contract.functions.head().call()
        tail = contract.functions.tail().call()
    else:
        shasta_abi = get_shasta_inbox_abi()
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=shasta_abi)
        head, tail = contract.functions.getForcedInclusionState().call()
        print("Forced Inclusion head:", head, "tail: ", tail)
    return head == tail

def check_empty_forced_inclusion_store(l1_client, env_vars):
    assert forced_inclusion_store_is_empty(l1_client, env_vars), "Forced inclusion store should be empty"
