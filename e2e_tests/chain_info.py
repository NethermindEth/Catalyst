from dataclasses import dataclass
from hexbytes import HexBytes
from taiko_inbox import get_last_batch_id
from utils import get_slot_in_epoch

@dataclass
class ChainInfo:
    """Chain Info"""
    # Forced inclusion sender nonce
    fi_sender_nonce: int
    batch_id: int
    block_number: int
    block_hash: HexBytes

    @classmethod
    def from_chain(cls, fi_account_address, l2_client_node1, l1_client, env_vars, beacon_client, verbose: bool = True):
        """Create ChainInfo instance from current chain state"""
        fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account_address)
        batch_id = get_last_batch_id(l1_client, env_vars)
        block_number = l2_client_node1.eth.block_number
        block_hash = l2_client_node1.eth.get_block(block_number).hash

        if verbose:
            print("----------------")
            print("Slot in epoch:", get_slot_in_epoch(beacon_client))
            print("FI sender nonce:", fi_sender_nonce)
            print("Batch ID:", batch_id)
            print("Block number:", block_number)
            print("Block hash:", block_hash.hex())

        return cls(
            fi_sender_nonce=fi_sender_nonce,
            batch_id=batch_id,
            block_number=block_number,
            block_hash=block_hash
        )

    def check_reorg(self, l2_client_node1):
        """Verify that the cached block hash matches the current chain state (detect reorgs)."""
        latest_block_number = l2_client_node1.eth.block_number
        assert self.block_number <= latest_block_number, (
            f"Cached block {self.block_number} is greater than the latest block {latest_block_number}"
        )
        current_block_hash = l2_client_node1.eth.get_block(self.block_number).hash
        assert self.block_hash == current_block_hash, (
            f"Reorg detected on block {self.block_number}: "
            f"prev hash {self.block_hash.hex()} cur hash {current_block_hash.hex()}"
        )