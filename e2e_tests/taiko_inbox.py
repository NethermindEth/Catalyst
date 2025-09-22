from web3 import Web3
import os
from dotenv import load_dotenv
import json

load_dotenv()

taiko_inbox_address = os.getenv("TAIKO_INBOX_ADDRESS")
if not taiko_inbox_address:
    raise Exception("Environment variable TAIKO_INBOX_ADDRESS not set")

with open("../whitelist/src/l1/abi/ITaikoInbox.json") as f:
    abi = json.load(f)

def get_last_batch_id(l1_client):
    contract = l1_client.eth.contract(address=taiko_inbox_address, abi=abi)
    result = contract.functions.getStats2().call()
    last_batch_id = result[0]
    print("last_batch_id:", last_batch_id)
    return last_batch_id
