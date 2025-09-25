from web3 import Web3
import json

with open("../whitelist/src/l1/abi/ITaikoInbox.json") as f:
    abi = json.load(f)

def get_last_batch_id(l1_client, taiko_inbox_address):
    contract = l1_client.eth.contract(address=taiko_inbox_address, abi=abi)
    result = contract.functions.getStats2().call()
    last_batch_id = result[0]
    return last_batch_id
