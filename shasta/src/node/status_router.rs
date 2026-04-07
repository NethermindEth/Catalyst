use crate::l1::execution_layer::ExecutionLayer;
use axum::{
    Router, extract::State, http::StatusCode, http::header, response::IntoResponse, routing::get,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct StatusState {
    el: Arc<ExecutionLayer>,
    slot_clock: Arc<common::l1::slot_clock::SlotClock>,
}

pub fn status_router(
    el: Arc<ExecutionLayer>,
    slot_clock: Arc<common::l1::slot_clock::SlotClock>,
) -> Router {
    let state = StatusState { el, slot_clock };
    Router::new()
        .route("/status", get(status_handler))
        .with_state(state)
}

async fn status_handler(State(state): State<StatusState>) -> impl IntoResponse {
    match state.slot_clock.get_current_slot() {
        Ok(l1_slot) => {
            let epoch = state.slot_clock.get_epoch_from_slot(l1_slot);
            let slot_of_epoch = state.slot_clock.slot_of_epoch(l1_slot);

            match state.slot_clock.get_current_l2_slot_within_l1_slot() {
                Ok(l2_slot) => {
                    let response = json!({
                        "epoch": epoch,
                        "slot_of_epoch": slot_of_epoch,
                        "l2_slot": l2_slot,
                    });

                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        response.to_string(),
                    )
                        .into_response()
                }
                Err(e) => {
                    let error_response = json!({
                        "error": format!("Failed to get L2 slot: {}", e),
                    });
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error_response.to_string(),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            let error_response = json!({
                "error": format!("Failed to get current slot: {}", e),
            });
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_response.to_string(),
            )
                .into_response()
        }
    }
}
