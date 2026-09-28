use serde::Serialize;
use sm_framing::{
    BufferBudget, FrameError, Phase, Reader, Writer, APPLICATION_LIMIT, HANDSHAKE_LIMIT,
    HOST_BUFFER_BUDGET, WRITER_SEND_CAPACITY,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
};
use tokio::{
    io::{duplex, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{advance, Duration},
};

fn wire(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}
async fn input(bytes: &[u8], phase: Phase) -> Result<Option<sm_framing::Frame>, FrameError> {
    input_with_budget(bytes, phase, BufferBudget::new()).await
}
async fn input_with_budget(
    bytes: &[u8],
    phase: Phase,
    budget: BufferBudget,
) -> Result<Option<sm_framing::Frame>, FrameError> {
    let (mut tx, rx) = duplex(bytes.len().max(1) + 4);
    tx.write_all(bytes).await.unwrap();
    drop(tx);
    Reader::new(rx, budget).receive(phase).await
}

#[tokio::test]
async fn s01_ipc_01_fragmentation_coalescing_utf8_and_lf() {
    let (mut tx, rx) = duplex(8);
    let mut reader = Reader::new(rx, BufferBudget::new());
    let task = tokio::spawn(async move {
        let one = reader.receive(Phase::Application).await.unwrap().unwrap();
        assert_eq!(one.as_bytes(), "{\"a\":\"é\\n中\"}".as_bytes());
        drop(one);
        let two = reader.receive(Phase::Application).await.unwrap().unwrap();
        assert_eq!(two.as_bytes(), b"[1]");
        drop(two);
        assert!(reader.receive(Phase::Application).await.unwrap().is_none());
    });
    for byte in wire("{\"a\":\"é\\n中\"}".as_bytes()) {
        tx.write_all(&[byte]).await.unwrap();
        tokio::task::yield_now().await;
    }
    tx.write_all(&wire(b"[1]")).await.unwrap();
    drop(tx);
    task.await.unwrap();
    let mut combined = wire(b"1");
    combined.extend_from_slice(&wire(b"2"));
    let (mut tx, rx) = duplex(16);
    tx.write_all(&combined).await.unwrap();
    drop(tx);
    let mut reader = Reader::new(rx, BufferBudget::new());
    assert_eq!(
        reader
            .receive(Phase::Application)
            .await
            .unwrap()
            .unwrap()
            .as_bytes(),
        b"1"
    );
    assert_eq!(
        reader
            .receive(Phase::Application)
            .await
            .unwrap()
            .unwrap()
            .as_bytes(),
        b"2"
    );
}

#[tokio::test]
async fn s01_ipc_02_bounds_bad_json_depth_and_eof() {
    let budget = BufferBudget::new();
    for (n, phase, code) in [
        (0, Phase::Application, FrameError::InvalidLength),
        (
            APPLICATION_LIMIT as u32 + 1,
            Phase::Application,
            FrameError::FrameTooLarge,
        ),
        (u32::MAX, Phase::Application, FrameError::FrameTooLarge),
        (
            HANDSHAKE_LIMIT as u32 + 1,
            Phase::Handshake,
            FrameError::FrameTooLarge,
        ),
    ] {
        let (mut tx, rx) = duplex(4);
        tx.write_all(&n.to_le_bytes()).await.unwrap();
        drop(tx);
        let mut reader = Reader::new(rx, budget.clone());
        assert!(matches!(reader.receive(phase).await, Err(e) if e == code));
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    }
    for (bytes, code) in [
        (vec![0xff, 0xfe], FrameError::InvalidUtf8),
        (b"{x}".to_vec(), FrameError::InvalidJson),
        (b"{} {}".to_vec(), FrameError::InvalidJson),
        (b"[\"unfinished]".to_vec(), FrameError::InvalidJson),
        (
            format!("{}0{}", "[".repeat(65), "]".repeat(65)).into_bytes(),
            FrameError::JsonTooDeep,
        ),
    ] {
        assert!(matches!(input(&wire(&bytes), Phase::Application).await, Err(e) if e == code));
    }
    let depth64 = format!(
        "{}\"escaped \\\" [ ] \\\\ braces {{ }}\"{}",
        "[".repeat(64),
        "]".repeat(64)
    );
    assert!(input(&wire(depth64.as_bytes()), Phase::Application)
        .await
        .unwrap()
        .is_some());
    assert!(input(&[], Phase::Application).await.unwrap().is_none());
    for bytes in [&[1u8][..], &[3, 0, 0, 0, b'['][..]] {
        assert!(matches!(
            input(bytes, Phase::Application).await,
            Err(FrameError::ProtocolTruncatedFrame)
        ));
    }
    let exact = format!("\"{}\"", "a".repeat(APPLICATION_LIMIT - 2));
    assert_eq!(
        input(&wire(exact.as_bytes()), Phase::Application)
            .await
            .unwrap()
            .unwrap()
            .as_bytes()
            .len(),
        APPLICATION_LIMIT
    );
    let hs = format!("\"{}\"", "a".repeat(HANDSHAKE_LIMIT - 2));
    assert_eq!(
        input(&wire(hs.as_bytes()), Phase::Handshake)
            .await
            .unwrap()
            .unwrap()
            .as_bytes()
            .len(),
        HANDSHAKE_LIMIT
    );
}

#[tokio::test]
async fn s01_ipc_02_aggregate_permits_block_payload_and_release_on_cancel() {
    let budget = BufferBudget::new();
    let payload = format!("\"{}\"", "x".repeat(APPLICATION_LIMIT - 2));
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(
            input_with_budget(
                &wire(payload.as_bytes()),
                Phase::Application,
                budget.clone(),
            )
            .await
            .unwrap()
            .unwrap(),
        );
    }
    assert_eq!(budget.available(), 0);
    let (mut tx, rx) = duplex(8);
    let budget2 = budget.clone();
    let task =
        tokio::spawn(async move { Reader::new(rx, budget2).receive(Phase::Application).await });
    tx.write_all(&(APPLICATION_LIMIT as u32).to_le_bytes())
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    // The budget must be acquired before even the first payload byte is consumed.
    tx.write_all(b"\"").await.unwrap();
    assert!(!task.is_finished());
    task.abort();
    let _ = task.await;
    assert_eq!(budget.available(), 0);
    drop(held);
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    let (mut tx, rx) = duplex(8);
    let budget2 = budget.clone();
    let task =
        tokio::spawn(async move { Reader::new(rx, budget2).receive(Phase::Application).await });
    tx.write_all(&wire(b"{}")).await.unwrap();
    assert!(task.await.unwrap().unwrap().is_some());
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    let (mut tx, rx) = duplex(8);
    let mut reader = Reader::new(rx, budget.clone());
    tx.write_all(&[3, 0, 0, 0, b'{']).await.unwrap();
    drop(tx);
    assert!(matches!(
        reader.receive(Phase::Application).await,
        Err(FrameError::ProtocolTruncatedFrame)
    ));
    assert!(reader.is_poisoned());
    assert!(matches!(
        reader.receive(Phase::Application).await,
        Err(FrameError::Poisoned)
    ));
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
}

#[tokio::test]
async fn s01_ipc_reader_cancel_mid_payload_poison_and_release() {
    let budget = BufferBudget::new();
    let (mut tx, rx) = duplex(8);
    let mut reader = Reader::new(rx, budget.clone());
    tx.write_all(&[4, 0, 0, 0, b'[']).await.unwrap();
    tokio::select! {
        biased;
        result = reader.receive(Phase::Application) => panic!("unexpected receive completion: {}", result.err().map_or("ok", FrameError::code)),
        () = async { tokio::task::yield_now().await; } => {}
    }
    assert!(reader.is_poisoned());
    assert!(matches!(
        reader.receive(Phase::Application).await,
        Err(FrameError::Poisoned)
    ));
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
}

#[tokio::test(start_paused = true)]
async fn s01_ipc_02_handshake_absolute_deadline_on_drip_and_stall() {
    let (mut tx, rx) = duplex(8);
    let task = tokio::spawn(async move {
        Reader::new(rx, BufferBudget::new())
            .receive(Phase::Handshake)
            .await
    });
    tokio::task::yield_now().await;
    for b in (5u32).to_le_bytes().iter().take(3) {
        tx.write_all(&[*b]).await.unwrap();
        advance(Duration::from_secs(1)).await;
    }
    assert!(matches!(
        task.await.unwrap(),
        Err(FrameError::DeadlineExceeded)
    ));
    let (_tx, rx) = duplex(8);
    let task = tokio::spawn(async move {
        Reader::new(rx, BufferBudget::new())
            .receive(Phase::Handshake)
            .await
    });
    tokio::task::yield_now().await;
    advance(Duration::from_secs(3)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(FrameError::DeadlineExceeded)
    ));
}

#[tokio::test]
async fn s01_ipc_writer_fifo_short_writes_backpressure_and_limits() {
    let budget = BufferBudget::new();
    let (tx, mut rx) = duplex(1);
    let writer = Writer::new(tx, budget.clone());
    let first = tokio::spawn({
        let w = writer.clone();
        async move { w.send(Phase::Application, &"first").await }
    });
    tokio::task::yield_now().await;
    let second = tokio::spawn({
        let w = writer.clone();
        async move { w.send(Phase::Application, &"second").await }
    });
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    let mut result = vec![0; wire(b"\"first\"").len() + wire(b"\"second\"").len()];
    rx.read_exact(&mut result).await.unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let mut expected = wire(b"\"first\"");
    expected.extend(wire(b"\"second\""));
    assert_eq!(result, expected);
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    assert!(matches!(
        writer
            .send(Phase::Handshake, &"a".repeat(HANDSHAKE_LIMIT))
            .await,
        Err(FrameError::FrameTooLarge)
    ));
    assert!(!writer.is_poisoned());
}

#[tokio::test]
async fn s01_ipc_writer_partial_cancel_poison_and_release_waiters() {
    let budget = BufferBudget::new();
    let (tx, mut rx) = duplex(1);
    let writer = Writer::new(tx, budget.clone());
    let send = tokio::spawn({
        let w = writer.clone();
        async move { w.send(Phase::Application, &"x".repeat(100)).await }
    });
    tokio::task::yield_now().await;
    let mut b = [0];
    rx.read_exact(&mut b).await.unwrap();
    let waiter = tokio::spawn({
        let w = writer.clone();
        async move { w.send(Phase::Application, &0).await }
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    send.abort();
    let _ = send.await;
    assert!(matches!(waiter.await.unwrap(), Err(FrameError::Poisoned)));
    assert!(writer.is_poisoned());
    assert!(matches!(
        writer.send(Phase::Application, &0).await,
        Err(FrameError::Poisoned)
    ));
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
}

type SendFuture = Pin<Box<dyn Future<Output = Result<(), FrameError>> + Send>>;

struct CountedPayload {
    serializations: Arc<AtomicUsize>,
    id: u8,
}
impl Serialize for CountedPayload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.serializations.fetch_add(1, Ordering::SeqCst);
        serializer.serialize_u8(self.id)
    }
}

#[tokio::test]
async fn s01_ipc_writer_admission_bounded_cancel_and_drain() {
    assert_eq!(WRITER_SEND_CAPACITY, 8);
    let budget = BufferBudget::new();
    let (tx, mut rx) = duplex(1);
    let writer = Writer::new(tx, budget.clone());
    let serializations = Arc::new(AtomicUsize::new(0));
    let first = tokio::spawn({
        let writer = writer.clone();
        let counter = serializations.clone();
        async move {
            writer
                .send(
                    Phase::Application,
                    &CountedPayload {
                        serializations: counter,
                        id: 0,
                    },
                )
                .await
        }
    });
    // The prefix byte proves the first send entered the write path and is stalled.
    let mut prefix_byte = [0];
    rx.read_exact(&mut prefix_byte).await.unwrap();
    let mut queued: Vec<SendFuture> = (1..8)
        .map(|id| {
            let writer = writer.clone();
            let counter = serializations.clone();
            Box::pin(async move {
                writer
                    .send(
                        Phase::Application,
                        &CountedPayload {
                            serializations: counter,
                            id,
                        },
                    )
                    .await
            }) as _
        })
        .collect();
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for future in &mut queued {
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(serializations.load(Ordering::SeqCst), 1);
    // 2000 distinct polled callers do not join an unbounded mutex/semaphore wait list.
    for _ in 0..2000 {
        let value = CountedPayload {
            serializations: serializations.clone(),
            id: 99,
        };
        let mut overflow = Box::pin(writer.send(Phase::Application, &value));
        assert!(matches!(
            overflow.as_mut().poll(&mut cx),
            Poll::Ready(Err(FrameError::QueueFull))
        ));
    }
    assert_eq!(serializations.load(Ordering::SeqCst), 1);
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET - APPLICATION_LIMIT);
    // Dropping one queued future (before first byte) returns exactly one slot.
    drop(queued.remove(3));
    assert!(!writer.is_poisoned());
    let mut replacement_send = Box::pin({
        let writer = writer.clone();
        let counter = serializations.clone();
        async move {
            writer
                .send(
                    Phase::Application,
                    &CountedPayload {
                        serializations: counter,
                        id: 9,
                    },
                )
                .await
        }
    });
    assert!(replacement_send.as_mut().poll(&mut cx).is_pending());
    assert!(matches!(
        Box::pin(writer.send(Phase::Application, &10))
            .as_mut()
            .poll(&mut cx),
        Poll::Ready(Err(FrameError::QueueFull))
    ));
    // Drain raw bytes including the already consumed prefix, checking every boundary/body.
    let read = tokio::spawn(async move {
        for (index, id) in [0u8, 1, 2, 3, 5, 6, 7, 9].iter().enumerate() {
            let mut prefix = [0; 4];
            if index == 0 {
                prefix[0] = prefix_byte[0];
                rx.read_exact(&mut prefix[1..]).await.unwrap();
            } else {
                rx.read_exact(&mut prefix).await.unwrap();
            }
            let mut body = vec![0; u32::from_le_bytes(prefix) as usize];
            rx.read_exact(&mut body).await.unwrap();
            assert_eq!(body, id.to_string().as_bytes());
        }
        rx
    });
    first.await.unwrap().unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for future in queued {
        tasks.spawn(future);
    }
    // Replacement is local and its prior mutex registration keeps it last.
    let replacement_task = tokio::spawn(replacement_send);
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
    replacement_task.await.unwrap().unwrap();
    let mut rx = read.await.unwrap();
    assert_eq!(serializations.load(Ordering::SeqCst), 8);
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    assert!(!writer.is_poisoned());
    // A drained writer admits new work.
    let next = tokio::spawn({
        let w = writer.clone();
        async move { w.send(Phase::Application, &11).await }
    });
    // Duplex has one byte of capacity; cancellation here would poison, so drain.
    let mut frame = [0; 6];
    rx.read_exact(&mut frame).await.unwrap();
    next.await.unwrap().unwrap();
    assert_eq!(frame.as_slice(), wire(b"11"));
}

#[tokio::test]
async fn s01_ipc_writer_poison_releases_all_admitted_waiters_and_race() {
    let budget = BufferBudget::new();
    let (tx, mut rx) = duplex(1);
    let writer = Writer::new(tx, budget.clone());
    let active = tokio::spawn({
        let writer = writer.clone();
        async move { writer.send(Phase::Application, &"x".repeat(100)).await }
    });
    let mut byte = [0];
    rx.read_exact(&mut byte).await.unwrap();
    let mut waiters: Vec<SendFuture> = (0..7)
        .map(|_| {
            let writer = writer.clone();
            Box::pin(async move { writer.send(Phase::Application, &0).await }) as _
        })
        .collect();
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for waiter in &mut waiters {
        assert!(waiter.as_mut().poll(&mut cx).is_pending());
    }
    assert!(matches!(
        Box::pin(writer.send(Phase::Application, &1))
            .as_mut()
            .poll(&mut cx),
        Poll::Ready(Err(FrameError::QueueFull))
    ));
    active.abort();
    // The abort may not have run yet: either refusal is safe, never a frame write.
    let race = writer.send(Phase::Application, &2).await.unwrap_err();
    assert!(matches!(race, FrameError::QueueFull | FrameError::Poisoned));
    let _ = active.await;
    assert!(writer.is_poisoned());
    for waiter in waiters {
        assert_eq!(waiter.await, Err(FrameError::Poisoned));
    }
    for _ in 0..8 {
        assert_eq!(
            writer.send(Phase::Application, &3).await,
            Err(FrameError::Poisoned)
        );
    }
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    // The first prefix byte is the only observed output; no queued body was written.
    assert_eq!(byte, [102]);
}

struct InvalidPayload;
impl Serialize for InvalidPayload {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "private synthetic serialization failure",
        ))
    }
}
#[tokio::test]
async fn s01_ipc_writer_payload_error_releases_admission_without_poison() {
    let (tx, mut rx) = duplex(8);
    let budget = BufferBudget::new();
    let writer = Writer::new(tx, budget.clone());
    for _ in 0..16 {
        assert_eq!(
            writer.send(Phase::Application, &InvalidPayload).await,
            Err(FrameError::InvalidJson)
        );
    }
    assert!(!writer.is_poisoned());
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    let send = tokio::spawn(async move { writer.send(Phase::Application, &4).await });
    let mut frame = [0; 5];
    rx.read_exact(&mut frame).await.unwrap();
    send.await.unwrap().unwrap();
    assert_eq!(frame.as_slice(), wire(b"4"));
}

struct FailAfterPrefix {
    calls: usize,
}
impl AsyncWrite for FailAfterPrefix {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.calls += 1;
        if self.calls == 1 {
            Poll::Ready(Ok(1.min(buf.len())))
        } else {
            Poll::Ready(Err(io::Error::other("sensitive synthetic write error")))
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn s01_ipc_writer_error_poison_and_safe_errors() {
    let budget = BufferBudget::new();
    let writer = Writer::new(FailAfterPrefix { calls: 0 }, budget.clone());
    let error = writer.send(Phase::Application, &42).await.unwrap_err();
    assert_eq!(error, FrameError::Io);
    assert_eq!(error.to_string(), "TRANSPORT_IO");
    assert!(writer.is_poisoned());
    assert!(matches!(
        writer.send(Phase::Application, &42).await,
        Err(FrameError::Poisoned)
    ));
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
}

struct Endless;
impl Serialize for Endless {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(None)?;
        for _ in 0..(APPLICATION_LIMIT / 2) {
            seq.serialize_element(&123456u32)?;
        }
        seq.end()
    }
}
#[tokio::test]
async fn s01_ipc_outbound_serializer_stops_at_cap_without_poison() {
    let (tx, _rx) = duplex(8);
    let budget = BufferBudget::new();
    let writer = Writer::new(tx, budget.clone());
    assert!(matches!(
        writer.send(Phase::Application, &Endless).await,
        Err(FrameError::FrameTooLarge)
    ));
    assert!(!writer.is_poisoned());
    assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
}
