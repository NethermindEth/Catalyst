use tracing_subscriber::{EnvFilter, filter::FilterFn, fmt, prelude::*};

pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("debug")
            .add_directive(
                "reqwest=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "hyper=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "alloy_transport=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "alloy_rpc_client=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "p2p_network=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "libp2p_gossipsub=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "discv5=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
            .add_directive(
                "netlink_proto=info"
                    .parse()
                    .expect("assert: can parse env filter directive"),
            )
    });

    // Create a custom formatter for heartbeat logs
    let heartbeat_format = fmt::format()
        .with_timer(fmt::time::time())
        .with_target(false)
        .with_level(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false);

    // Create a standard formatter for all other logs
    let standard_format = fmt::format()
        .with_timer(fmt::time::time())
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false);

    // Create a layered subscriber that uses different formatters based on the target
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::Layer::default()
                .with_writer(std::io::stdout)
                .event_format(standard_format)
                .with_filter(FilterFn::new(|metadata: &tracing::Metadata<'_>| {
                    !metadata.target().contains("heartbeat")
                })),
        )
        .with(
            fmt::Layer::default()
                .with_writer(std::io::stdout)
                .event_format(heartbeat_format)
                .with_filter(FilterFn::new(|metadata: &tracing::Metadata<'_>| {
                    metadata.target().contains("heartbeat")
                })),
        );

    subscriber.init();
}
