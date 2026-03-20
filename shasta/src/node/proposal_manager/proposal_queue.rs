use std::collections::VecDeque;

use super::proposal::{Proposal, Proposals};

pub(super) struct ProposalQueue {
    proposals: VecDeque<Proposal>,
}

impl ProposalQueue {
    pub fn new() -> Self {
        Self {
            proposals: VecDeque::new(),
        }
    }

    pub fn push(&mut self, proposal: Proposal) {
        self.proposals.push_back(proposal);
    }

    pub fn len(&self) -> u64 {
        self.proposals.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    pub fn remove_confirmed(&mut self) {
        if self
            .proposals
            .front()
            .is_some_and(|p| p.pending_confirmation)
        {
            self.proposals.pop_front();
        }
    }

    pub fn mark_front_for_resubmit(&mut self) {
        if let Some(proposal) = self.proposals.front_mut() {
            if !proposal.pending_confirmation {
                tracing::error!(
                    "There is no pending confirmation proposal to mark as not confirmed."
                );
            }
            proposal.pending_confirmation = false;
        }
    }

    pub fn take_all(&mut self) -> VecDeque<Proposal> {
        std::mem::take(&mut self.proposals)
    }

    pub fn prepend(&mut self, mut proposals: Proposals) {
        proposals.append(&mut self.proposals);
        self.proposals = proposals;
    }

    pub fn front_mut(&mut self) -> Option<&mut Proposal> {
        self.proposals.front_mut()
    }
}
