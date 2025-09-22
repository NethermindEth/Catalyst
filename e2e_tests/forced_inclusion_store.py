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

def forced_inclusion_store_is_empty(l1_client, forced_inclusion_address):
    contract = l1_client.eth.contract(address=forced_inclusion_address, abi=abi)
    head = contract.functions.head().call()
    tail = contract.functions.tail().call()
    print("Forced Inclusion head:", head, "tail: ", tail)
    assert head == tail, "Forced inclusion is not empty"
