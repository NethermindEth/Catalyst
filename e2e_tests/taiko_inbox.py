#import requests
from web3 import Web3
import os
from dotenv import load_dotenv
#import sys
#from utils import *

load_dotenv()

taiko_inbox_address = os.getenv("TAIKO_INBOX_ADDRESS")
if not taiko_inbox_address:
    raise Exception("Environment variable TAIKO_INBOX_ADDRESS not set")

abi = [
    {
        "inputs": [],
        "name": "getStats2",
        "outputs": [
            {
                "components": [
                    {"internalType": "uint64", "name": "value0", "type": "uint64"},
                    {"internalType": "uint64", "name": "value1", "type": "uint64"},
                    {"internalType": "bool", "name": "value2", "type": "bool"},
                    {"internalType": "uint56", "name": "value3", "type": "uint56"},
                    {"internalType": "uint64", "name": "value4", "type": "uint64"},
                ],
                "internalType": "struct YourStructName",
                "name": "",
                "type": "tuple",
            }
        ],
        "stateMutability": "view",
        "type": "function",
    }
]

def get_last_batch_id(l1_client):
    contract = l1_client.eth.contract(address=taiko_inbox_address, abi=abi)
    result = contract.functions.getStats2().call()
    last_batch_id = result[0]
    print("last_batch_id:", last_batch_id)
    return last_batch_id
