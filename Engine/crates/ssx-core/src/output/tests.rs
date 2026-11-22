use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use super::*;

type Seen = Arc<Mutex<Vec<(Stream, Vec<u8>, String)>>>;

fn sink(seen: &Seen, gate: &Arc<(Mutex<bool>, Condvar)>) -> impl FnMut(Stream, &[u8]) -> io::Result<()> + use<> {
    let (seen, gate) = (Arc::clone(seen), Arc::clone(gate));
    move |stream, bytes| {
        let (open, changed) = &*gate;
        let mut o = open.lock().unwrap();
        while !*o {
            o = changed.wait(o).unwrap();
        }
        drop(o);
        let name = std::thread::current().name().unwrap_or("").to_string();
        seen.lock().unwrap().push((stream, bytes.to_vec(), name));
        Ok(())
    }
}

fn gate(open: bool) -> Arc<(Mutex<bool>, Condvar)> {
    Arc::new((Mutex::new(open), Condvar::new()))
}

fn open(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

fn taken(out: &Output) {
    for _ in 0..1000 {
        if out.lock().queue.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the writer took nothing in 5 s");
}

fn text(seen: &Seen) -> String {
    seen.lock().unwrap().iter().map(|(_, b, _)| String::from_utf8_lossy(b).into_owned()).collect()
}

#[test]
fn a_line_is_written_by_the_writer_in_the_order_said() {
    let (seen, g) = (Seen::default(), gate(true));
    let out = Output::new();
    let running = out.start("test-writer", sink(&seen, &g)).unwrap();
    out.line(Stream::Err, Keep::Droppable, b"one\n".to_vec()).unwrap();
    out.line(Stream::Out, Keep::Always, b"two\n".to_vec()).unwrap();
    out.line(Stream::Err, Keep::Always, b"three\n".to_vec()).unwrap();
    out.flush();
    let got = seen.lock().unwrap().clone();
    assert!(got.iter().all(|(_, _, t)| t == "test-writer"), "{got:?}");
    let order: Vec<(Stream, String)> =
        got.iter().map(|(s, b, _)| (*s, String::from_utf8_lossy(b).into_owned())).collect();
    let lines: Vec<(Stream, &str)> =
        order.iter().flat_map(|(s, t)| t.lines().map(move |l| (*s, l))).collect::<Vec<_>>();
    assert_eq!(lines, [(Stream::Err, "one"), (Stream::Out, "two"), (Stream::Err, "three")]);
    drop(running);
}

#[test]
fn lines_past_the_bound_are_left_out_counted_and_said_kept_ones_never() {
    let (seen, g) = (Seen::default(), gate(false));
    let out = Output::new();
    let running = out.start("test-writer", sink(&seen, &g)).unwrap();
    out.line(Stream::Err, Keep::Always, b"first\n".to_vec()).unwrap();
    taken(&out);
    let mut line = vec![b'x'; 1023];
    line.push(b'\n');
    let fits = QUEUE_BYTES / line.len();
    for _ in 0..fits + 100 {
        out.line(Stream::Err, Keep::Droppable, line.clone()).unwrap();
    }
    out.line(Stream::Err, Keep::Always, b"kept\n".to_vec()).unwrap();
    for _ in 0..7 {
        out.line(Stream::Err, Keep::Droppable, line.clone()).unwrap();
    }
    open(&g);
    out.flush();
    drop(running);
    let all = text(&seen);
    assert!(all.starts_with("first\n"));
    let xs = all.lines().filter(|l| l.starts_with('x')).count();
    assert_eq!(xs, fits);
    let notices: Vec<&str> = all.lines().filter(|l| l.contains("left out here")).collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains(" WARN ssx_core::output: log: 100 lines of the log (102400 bytes)"), "{}", notices[0]);
    assert!(notices[1].contains("log: 7 lines of the log (7168 bytes)"), "{}", notices[1]);
    let at = |p: &str| all.find(p).unwrap();
    assert!(at(notices[0]) < at("kept\n") && at("kept\n") < at(notices[1]));
}

#[test]
fn a_kept_line_waits_for_room_and_is_written() {
    let (seen, g) = (Seen::default(), gate(false));
    let out = Output::new();
    let running = out.start("test-writer", sink(&seen, &g)).unwrap();
    out.line(Stream::Err, Keep::Always, b"first\n".to_vec()).unwrap();
    taken(&out);
    let mut big = vec![b'k'; (1 << 20) - 1];
    big.push(b'\n');
    for _ in 0..QUEUE_BYTES_KEPT / big.len() {
        out.line(Stream::Err, Keep::Always, big.clone()).unwrap();
    }
    let o = Arc::clone(&out);
    let waiting = std::thread::spawn(move || o.line(Stream::Err, Keep::Always, b"last\n".to_vec()));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!waiting.is_finished(), "a kept line past the bound did not wait for room");
    open(&g);
    waiting.join().unwrap().unwrap();
    out.flush();
    drop(running);
    let all = text(&seen);
    assert_eq!(all.lines().filter(|l| l.starts_with('k')).count(), QUEUE_BYTES_KEPT / big.len());
    assert!(all.ends_with("last\n") && !all.contains("left out"));
}

#[test]
fn the_guard_writes_everything_then_the_lines_go_straight_out() {
    let (seen, g) = (Seen::default(), gate(true));
    let out = Output::new();
    let running = out.start("test-writer", sink(&seen, &g)).unwrap();
    for i in 0..1000 {
        out.line(Stream::Err, Keep::Droppable, format!("line {i}\n").into_bytes()).unwrap();
    }
    drop(running);
    assert_eq!(text(&seen).lines().count(), 1000);
    assert_eq!(out.line(Stream::Err, Keep::Always, b"after\n".to_vec()), Err(b"after\n".to_vec()));
    let again = out.start("test-writer", sink(&seen, &g)).unwrap();
    out.line(Stream::Err, Keep::Always, b"again\n".to_vec()).unwrap();
    drop(again);
    assert!(text(&seen).ends_with("again\n"));
}

#[test]
fn the_log_time_format() {
    assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00.000000Z");
    assert_eq!(utc(UNIX_EPOCH + Duration::from_micros(1_763_271_205_233_896)), "2025-11-16T05:33:25.233896Z");
    assert_eq!(utc(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00.000000Z");
    assert_eq!(utc(UNIX_EPOCH + Duration::from_secs(4_102_444_799)), "2099-12-31T23:59:59.000000Z");
}

#[test]
fn flush_waits_for_every_started_output() {
    let (seen, g) = (Seen::default(), gate(false));
    let out = Output::new();
    let running = out.start("test-writer", sink(&seen, &g)).unwrap();
    out.line(Stream::Err, Keep::Droppable, b"held\n".to_vec()).unwrap();
    let flushing = std::thread::spawn(flush);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!flushing.is_finished(), "the module's flush did not wait for a started output's lines");
    open(&g);
    flushing.join().unwrap();
    assert_eq!(text(&seen), "held\n");
    drop(running);
    drop(out);
    flush();
}
