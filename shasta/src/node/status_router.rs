use crate::l1::execution_layer::ExecutionLayer;
use axum::{Router, extract::State, http::header, response::IntoResponse, routing::get};
use common::l1::traits::ELTrait;
use pacaya::l1::PreconfOperator;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct StatusState {
    el: Arc<ExecutionLayer>,
    slot_clock: Arc<common::l1::slot_clock::SlotClock>,
    preconfer_address: String,
}

pub fn status_router(
    el: Arc<ExecutionLayer>,
    slot_clock: Arc<common::l1::slot_clock::SlotClock>,
) -> Router {
    let preconfer_address = el.common().preconfer_address().to_string();
    let state = StatusState {
        el,
        slot_clock,
        preconfer_address,
    };
    Router::new()
        .route("/status", get(status_handler))
        .with_state(state)
}

async fn status_handler(State(state): State<StatusState>) -> impl IntoResponse {
    let mut errors: Vec<String> = vec![];

    // L1 slot
    let l1_slot = match state.slot_clock.get_current_slot() {
        Ok(slot) => Some(slot),
        Err(e) => {
            errors.push(format!("Failed to get current slot: {}", e));
            None
        }
    };

    // Epoch + slot_of_epoch
    let (epoch, slot_of_epoch) = match l1_slot {
        Some(slot) => {
            let epoch = state.slot_clock.get_epoch_from_slot(slot);
            let slot_of_epoch = state.slot_clock.slot_of_epoch(slot);
            (Some(epoch), Some(slot_of_epoch))
        }
        None => (None, None),
    };

    // Slot begin timestamp
    let slot_begin = match state.slot_clock.get_current_slot_begin_timestamp() {
        Ok(ts) => Some(ts),
        Err(e) => {
            errors.push(format!("Failed to get slot begin timestamp: {}", e));
            None
        }
    };

    // Operators
    let (current_operator, next_operator) = match slot_begin {
        Some(slot_begin) => {
            match state
                .el
                .get_operators_for_current_and_next_epoch(slot_begin)
                .await
            {
                Ok(cache) => (Some(cache.current_operator()), Some(cache.next_operator())),
                Err(e) => {
                    let msg = format!("Failed to get operators: {}", e);
                    errors.push(msg);
                    (None, None)
                }
            }
        }
        _ => (None, None),
    };

    // L2 slot
    let l2_slot = match state.slot_clock.get_current_l2_slot_within_l1_slot() {
        Ok(slot) => Some(slot),
        Err(e) => {
            errors.push(format!("Failed to get L2 slot: {}", e));
            None
        }
    };

    let response = json!({
        "fork": "shasta",
        "epoch": epoch,
        "l1_slot": slot_of_epoch,
        "l2_slot": l2_slot,
        "current_operator": current_operator,
        "next_operator": next_operator,
        "preconfer_address": state.preconfer_address,
        "errors": errors, // <-- key change
    });

    (
        [(header::CONTENT_TYPE, "application/json")],
        response.to_string(),
    )
        .into_response()
}
