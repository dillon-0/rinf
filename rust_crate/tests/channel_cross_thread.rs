/// Reproduces the pattern used in the Flutter hub: a tokio `current_thread`
/// runtime spawns a task that calls `recv().await`, then a foreign thread
/// calls `sender.send()`. The recv should complete within milliseconds.
///
/// If this test passes but the Flutter integration test fails, the issue
/// is in the Flutter test runner environment, not in rinf's channel or tokio.
use rinf::signal_channel;
use std::time::Duration;

#[test]
fn cross_thread_send_wakes_recv() {
    let (sender, receiver) = signal_channel::<String>();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        // Clone receiver — same as get_dart_signal_receiver() does.
        let active_receiver = receiver.clone();

        // Spawn the "handler" task — it awaits a signal.
        let handle = tokio::task::spawn(async move {
            active_receiver.recv().await
        });

        // Simulate the FFI thread sending a signal after a short delay.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            sender.send("hello from FFI thread".to_string());
        });

        // Wait for the handler to receive — should complete in ~50ms.
        let result = tokio::time::timeout(Duration::from_secs(3), handle)
            .await
            .expect("handler task should complete within 3s")
            .expect("handler task should not panic");

        assert_eq!(result, Some("hello from FFI thread".to_string()));
    });
}

#[test]
fn message_queued_before_recv_is_found() {
    // Send the message BEFORE the receiver clones and polls.
    // This is the "signal sent before handler starts" race.
    let (sender, receiver) = signal_channel::<u32>();
    sender.send(42);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        // Clone — this is what get_dart_signal_receiver() does.
        let active = receiver.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            active.recv(),
        )
        .await
        .expect("should complete immediately");

        assert_eq!(result, Some(42));
    });
}

/// Same as cross_thread_send_wakes_recv, but the runtime runs on a
/// SPAWNED thread (like rinf's start_rust_logic_real does), and send()
/// happens from the main thread (like the Dart FFI thread).
/// This mirrors the actual rinf architecture more closely.
#[test]
fn spawned_thread_runtime_receives_from_main_thread() {
    let (sender, receiver) = signal_channel::<String>();

    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    // Spawn a thread that runs the tokio runtime — like rinf does.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let active = receiver.clone();
            let handle = tokio::task::spawn(async move {
                active.recv().await
            });

            // Signal that the handler is spawned.
            ready_tx.send(()).unwrap();

            // Wait for the handler to complete.
            let result = tokio::time::timeout(Duration::from_secs(3), handle)
                .await
                .expect("should complete within 3s")
                .expect("should not panic");

            result_tx.send(result).unwrap();
        });
    });

    // Wait for the runtime thread to be ready.
    ready_rx.recv().unwrap();

    // Small delay to ensure recv() has been polled and waker stored.
    std::thread::sleep(Duration::from_millis(50));

    // Send from the "main" thread (like Dart FFI).
    sender.send("from main thread".to_string());

    let result = result_rx.recv_timeout(Duration::from_secs(5))
        .expect("result should arrive within 5s");
    assert_eq!(result, Some("from main thread".to_string()));
}
