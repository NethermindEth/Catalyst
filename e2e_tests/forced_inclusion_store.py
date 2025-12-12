from web3 import Web3
import json

pacaya_abi = [
  {
    "inputs": [],
    "name": "head",
    "outputs": [
      {
        "internalType": "uint64",
        "name": "",
        "type": "uint64"
      }
    ],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "inputs": [],
    "name": "tail",
    "outputs": [
      {
        "internalType": "uint64",
        "name": "",
        "type": "uint64"
      }
    ],
    "stateMutability": "view",
    "type": "function"
  }
]

shasta_abi = [
    {
        "inputs": [],
        "name": "getForcedInclusionState",
        "outputs": [
            {"internalType": "uint48", "name": "head_", "type": "uint48"},
            {"internalType": "uint48", "name": "tail_", "type": "uint48"},
            {"internalType": "uint48", "name": "lastProcessedAt_", "type": "uint48"}
        ],
        "stateMutability": "view",
        "type": "function"
    },
]

def get_forced_inclusion_store_head(l1_client, env_vars):
    if env_vars.is_pacaya():
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=pacaya_abi)
        head = contract.functions.head().call()
        return int(head)
    else:
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=shasta_abi)
        head, tail, last_processed_at = contract.functions.getForcedInclusionState().call()
        return int(head)

def forced_inclusion_store_is_empty(l1_client, env_vars):
    if env_vars.is_pacaya():
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=pacaya_abi)
        head = contract.functions.head().call()
        tail = contract.functions.tail().call()
    else:
        contract = l1_client.eth.contract(address=env_vars.forced_inclusion_store_address, abi=shasta_abi)
        head, tail, last_processed_at = contract.functions.getForcedInclusionState().call()
        print("Forced Inclusion head:", head, "tail: ", tail)
    return head == tail

def check_empty_forced_inclusion_store(l1_client, env_vars):
    assert forced_inclusion_store_is_empty(l1_client, env_vars), "Forced inclusion store should be empty"
