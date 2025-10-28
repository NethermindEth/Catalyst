from web3 import Web3
import json

abi = [
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

def get_forced_inclusion_store_head(l1_client, forced_inclusion_address):
    contract = l1_client.eth.contract(address=forced_inclusion_address, abi=abi)
    head = contract.functions.head().call()
    return int(head)

def forced_inclusion_store_is_empty(l1_client, forced_inclusion_address):
    contract = l1_client.eth.contract(address=forced_inclusion_address, abi=abi)
    head = contract.functions.head().call()
    tail = contract.functions.tail().call()
    print("Forced Inclusion head:", head, "tail: ", tail)
    return head == tail

def check_empty_forced_inclusion_store(l1_client, env_vars):
    assert forced_inclusion_store_is_empty(l1_client, env_vars.forced_inclusion_store_address), "Forced inclusion store should be empty"
