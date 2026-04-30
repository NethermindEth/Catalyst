pub mod bindings;

/// Selector for `Bridge.sendMessage((Message))`. Used by the L1 / L2 callback
/// simulation to detect outbound bridge messages in a call trace.
pub const SEND_MESSAGE_SELECTOR: [u8; 4] = [0x1b, 0xdb, 0x00, 0x37];
