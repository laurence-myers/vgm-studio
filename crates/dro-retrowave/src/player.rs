//! Playback on a wall clock.
//!
//! The emulated backend is paced by the sound card asking for samples. Hardware
//! output has no such pull -- the audio never passes through this program -- so a
//! thread walks the same [`PlayerEngine`] against [`Instant`] instead, handing
//! the register writes it produces to the device as they fall due.

use std::{
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use dro_core::Song;
use dro_synth::{
    NATIVE_SAMPLE_RATE,
    engine::{LoopConfig, Muting, Panning, PlayerEngine, Position},
};

use crate::{chip::SerialOpl3Chip, device::Device};

/// How much song time each pass through the loop covers.
///
/// Register writes land within about this much of their ideal moment, so it
/// wants to be short; every pass costs a sleep and a serial write, so it does
/// not want to be shorter than it must. 64 frames is 1.3 ms at the OPL's native
/// rate, which is below the threshold of hearing for note timing.
const QUANTUM_FRAMES: usize = 64;

/// [`QUANTUM_FRAMES`] as a duration.
const QUANTUM: Duration =
    Duration::from_nanos((QUANTUM_FRAMES as u64 * 1_000_000_000) / NATIVE_SAMPLE_RATE as u64);

/// How far behind the clock the pump tolerates before giving up on catching up.
///
/// Small overruns are made up by sleeping less on the next pass, which is what
/// keeps timing honest over a long song. A gap this large means something
/// stopped the world -- the machine slept, most likely -- and racing through the
/// backlog would just be noise.
const MAX_LAG: Duration = Duration::from_millis(250);

/// Control messages for the pump thread.
#[derive(Debug, Clone, Copy)]
enum Command {
    Play,
    Pause,
    SeekMs(u32),
    SeekPos(usize),
    Rewind,
    SetMuting(Muting),
    SetPanning(Panning),
    SetLoop(Option<LoopConfig>),
}

/// Playback state the pump publishes for the UI thread to poll.
#[derive(Debug, Default)]
struct SharedState {
    frames_rendered: AtomicU64,
    next_instruction: AtomicUsize,
    finished: AtomicBool,
    loop_iteration: AtomicU32,
    /// Set when the pump has stopped for good, so the transport can leave the
    /// "playing" state instead of showing a frozen cursor.
    stopped: AtomicBool,
    /// The first error the pump hit, waiting to be shown once.
    error: Mutex<Option<String>>,
}

/// One song, playing on real hardware.
///
/// Mirrors `NativeAudio`'s shape so the two backends can sit behind one
/// interface. Dropping this stops the pump and silences the chip; to keep the
/// port for the next song, take the device back with [`Self::into_device`].
#[derive(Debug)]
pub struct RetroWaveAudio {
    commands: rtrb::Producer<Command>,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<Option<Device>>>,
}

impl RetroWaveAudio {
    /// Starts a pump for `song` on `device`, paused.
    ///
    /// Takes ownership of the device for as long as the song is loaded; get it
    /// back with [`Self::into_device`] rather than reopening the port, which
    /// costs a chip reset and can fail outright on a port only just closed.
    #[must_use]
    pub fn new(device: Device, song: Arc<Song>) -> Self {
        let (commands, consumer) = rtrb::RingBuffer::new(64);
        let shared = Arc::new(SharedState::default());
        let stop = Arc::new(AtomicBool::new(false));

        let pump = thread::Builder::new()
            .name("retrowave-pump".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                move || run_pump(device, song, consumer, shared, stop)
            })
            .expect("spawning a thread");

        Self {
            commands,
            shared,
            stop,
            pump: Some(pump),
        }
    }

    /// Starts, or resumes, playback.
    pub fn play(&mut self) {
        self.send(Command::Play);
    }

    /// Stops advancing and releases every sounding note.
    pub fn pause(&mut self) {
        self.send(Command::Pause);
    }

    /// Seeks to the instruction playing at `ms`.
    pub fn seek_ms(&mut self, ms: u32) {
        self.send(Command::SeekMs(ms));
    }

    /// Seeks to instruction `pos`.
    pub fn seek_pos(&mut self, pos: usize) {
        self.send(Command::SeekPos(pos));
    }

    /// Returns to the start of the song.
    pub fn rewind(&mut self) {
        self.send(Command::Rewind);
    }

    /// Replaces the channel/percussion muting.
    pub fn set_muting(&mut self, muting: Muting) {
        self.send(Command::SetMuting(muting));
    }

    /// Replaces the per-channel panning.
    ///
    /// Only its effect on the song's own writes reaches the hardware: the
    /// emulator's panpot registers do not exist on a YMF262.
    pub fn set_panning(&mut self, panning: Panning) {
        self.send(Command::SetPanning(panning));
    }

    /// Sets (or clears) the region playback loops over.
    pub fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.send(Command::SetLoop(config));
    }

    /// The rate the engine steps at: always the OPL's native rate, since no
    /// sound card is involved to impose one.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        NATIVE_SAMPLE_RATE
    }

    /// The most recent position the pump published.
    #[must_use]
    pub fn position(&self) -> Position {
        Position::looping(
            self.shared.frames_rendered.load(Ordering::Relaxed),
            NATIVE_SAMPLE_RATE,
            self.shared.next_instruction.load(Ordering::Relaxed),
            self.shared.loop_iteration.load(Ordering::Relaxed),
        )
    }

    /// Whether the song has played to the end, or the pump has stopped.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed) || self.shared.stopped.load(Ordering::Relaxed)
    }

    /// Takes the pump's error, if it hit one. Reported once.
    pub fn take_error(&mut self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }

    /// Stops the pump and hands back the device, silenced.
    ///
    /// `None` only if the pump thread panicked, in which case the port is gone
    /// with it.
    #[must_use]
    pub fn into_device(mut self) -> Option<Device> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Option<Device> {
        self.stop.store(true, Ordering::Relaxed);
        self.pump.take()?.join().ok().flatten()
    }

    fn send(&mut self, command: Command) {
        if self.commands.push(command).is_err() {
            log::warn!("the RetroWave command queue is full; dropping a control command");
        }
    }
}

impl Drop for RetroWaveAudio {
    fn drop(&mut self) {
        // Whatever happens to the device, the chip must not be left sounding.
        drop(self.shutdown());
    }
}

/// The pump thread's body: set up, run, and silence the chip on the way out.
fn run_pump(
    mut device: Device,
    song: Arc<Song>,
    consumer: rtrb::Consumer<Command>,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
) -> Option<Device> {
    let chip = SerialOpl3Chip::new(song.opl_type);
    let mut engine = PlayerEngine::with_chip(song, chip, NATIVE_SAMPLE_RATE);

    // A panic in here would otherwise leave the chip holding whatever it was
    // playing -- a real one does not stop when the program does.
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        pump_loop(&mut device, &mut engine, consumer, &shared, &stop)
    }));

    match outcome {
        Ok(Err(error)) => {
            log::error!("RetroWave playback stopped: {error}");
            if let Ok(mut slot) = shared.error.lock() {
                slot.get_or_insert_with(|| error.to_string());
            }
            shared.stopped.store(true, Ordering::Relaxed);
            // The port is most likely gone; do not try to write to it again.
            return None;
        }
        Err(_) => log::error!("the RetroWave pump thread panicked"),
        Ok(Ok(())) => {}
    }
    shared.stopped.store(true, Ordering::Relaxed);

    engine.chip_mut().mute_sweep();
    // Best effort: if this fails the port is already gone, and a hard reset
    // belongs to closing the device rather than unloading a song.
    let _ = flush(&mut device, &mut engine);
    Some(device)
}

fn pump_loop(
    device: &mut Device,
    engine: &mut PlayerEngine<Arc<Song>, SerialOpl3Chip>,
    mut consumer: rtrb::Consumer<Command>,
    shared: &SharedState,
    stop: &AtomicBool,
) -> Result<(), crate::device::Error> {
    let mut playing = false;
    let mut was_finished = false;
    let mut scratch = vec![0i16; QUANTUM_FRAMES * 2];
    let mut deadline = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let mut reconcile = false;

        while let Ok(command) = consumer.pop() {
            match command {
                Command::Play => {
                    if !playing {
                        playing = true;
                        // The hardware has been silent (or stale) while paused;
                        // bring it up to whatever the song now expects.
                        reconcile = true;
                    }
                }
                Command::Pause => {
                    if playing {
                        playing = false;
                        engine.chip_mut().release_all_notes();
                    }
                }
                Command::SeekMs(ms) => {
                    engine.seek_to_ms(ms);
                    reconcile = true;
                }
                Command::SeekPos(pos) => {
                    engine.seek_to_pos(pos);
                    reconcile = true;
                }
                Command::Rewind => {
                    engine.rewind();
                    reconcile = true;
                }
                Command::SetMuting(muting) => engine.set_muting(muting),
                Command::SetPanning(panning) => {
                    engine.set_panning(panning);
                    reconcile = true;
                }
                Command::SetLoop(config) => engine.set_loop(config),
            }
        }

        // Reconciling while paused would undo the pause: the engine's seek
        // replays key-on bits, and a real chip would start sounding them. The
        // shadow keeps the intent; resuming plays it out.
        if reconcile && playing {
            engine.chip_mut().materialize();
        }

        // Unconditional: writes made while paused -- released notes, a channel
        // just muted -- have to reach the device too.
        flush(device, engine)?;

        if playing && !was_finished {
            engine.render(&mut scratch);
            flush(device, engine)?;

            let position = engine.position();
            shared
                .frames_rendered
                .store(position.frames_rendered, Ordering::Relaxed);
            shared
                .next_instruction
                .store(position.next_instruction, Ordering::Relaxed);
            shared
                .loop_iteration
                .store(position.loop_iteration, Ordering::Relaxed);
        }

        let finished = engine.is_finished();
        if finished && !was_finished {
            // Sweeping through the chip, not the device, so the hardware model
            // stays truthful and playing the song again reconstructs it fully.
            engine.chip_mut().mute_sweep();
            flush(device, engine)?;
        }
        was_finished = finished;
        shared.finished.store(finished, Ordering::Relaxed);

        deadline += QUANTUM;
        let now = Instant::now();
        if let Some(remaining) = deadline.checked_duration_since(now) {
            // Absolute deadlines, so a slow pass is made up by the next one
            // rather than accumulating into drift. Since Rust 1.75 this is
            // sub-millisecond accurate on Windows, which is finer than the
            // quantum -- no spin-waiting needed.
            thread::sleep(remaining);
        } else if now.duration_since(deadline) > MAX_LAG {
            deadline = now;
        }
    }

    Ok(())
}

/// Hands whatever the chip has queued to the device.
fn flush(
    device: &mut Device,
    engine: &mut PlayerEngine<Arc<Song>, SerialOpl3Chip>,
) -> Result<(), crate::device::Error> {
    let chip = engine.chip_mut();
    chip.seal();
    if chip.wire().is_empty() {
        return Ok(());
    }
    let result = device.send(chip.wire());
    chip.clear_wire();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::SerialIo;
    use dro_core::{DroDataV2, OplType};
    use std::{
        io,
        sync::mpsc::{Sender, channel},
    };

    /// Reports every write with the time it arrived, so tests can check pacing.
    #[derive(Debug)]
    struct TimedIo {
        events: Sender<(Instant, Vec<u8>)>,
    }

    impl SerialIo for TimedIo {
        fn write_all(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
            let _ = self.events.send((Instant::now(), bytes.to_vec()));
            Ok(())
        }

        fn flush(&mut self) -> Result<(), io::Error> {
            Ok(())
        }
    }

    /// A song that writes a register, waits `delay_ms` (at most 256), then
    /// writes another.
    fn two_writes_apart(delay_ms: u32) -> Arc<Song> {
        const SHORT_DELAY: u8 = 0xFE;
        const LONG_DELAY: u8 = 0xFF;
        let data = vec![
            0x00,
            0x01, // codemap[0], so register 0x20 = 0x01
            SHORT_DELAY,
            (delay_ms - 1) as u8,
            0x00,
            0x02, // register 0x20 = 0x02
        ];
        Arc::new(Song::dro_v2(
            "test".to_owned(),
            DroDataV2::new(data, vec![0x20, 0x40], SHORT_DELAY, LONG_DELAY)
                .expect("a well-formed fixture"),
            delay_ms,
            OplType::Opl3,
        ))
    }

    /// A song that keys a note on and holds it for `hold_ms` (at most 256).
    fn held_note(hold_ms: u32) -> Arc<Song> {
        const SHORT_DELAY: u8 = 0xFE;
        const LONG_DELAY: u8 = 0xFF;
        let data = vec![
            0x00,
            0x31, // codemap[0], so register 0xB0 = key on, block 4
            SHORT_DELAY,
            (hold_ms - 1) as u8,
        ];
        Arc::new(Song::dro_v2(
            "held.dro".to_owned(),
            DroDataV2::new(data, vec![0xB0, 0x20], SHORT_DELAY, LONG_DELAY)
                .expect("a well-formed fixture"),
            hold_ms,
            OplType::Opl3,
        ))
    }

    fn device_with(events: Sender<(Instant, Vec<u8>)>) -> Device {
        Device::with_io(Box::new(TimedIo { events }), "MOCK".to_owned()).expect("bring-up")
    }

    /// Collects everything written until the wire stays quiet for `quiet_for`.
    fn drain_burst(
        rx: &std::sync::mpsc::Receiver<(Instant, Vec<u8>)>,
        quiet_for: Duration,
    ) -> (Vec<u8>, Option<Instant>) {
        let mut bytes = Vec::new();
        let mut last = None;
        while let Ok((at, chunk)) = rx.recv_timeout(quiet_for) {
            bytes.extend_from_slice(&chunk);
            last = Some(at);
        }
        (bytes, last)
    }

    #[test]
    fn a_paused_pump_leaves_the_device_alone() {
        let (tx, rx) = channel();
        let device = device_with(tx);
        let audio = RetroWaveAudio::new(device, two_writes_apart(50));
        // Drain the bring-up traffic, then watch a while without pressing play.
        thread::sleep(Duration::from_millis(30));
        while rx.try_recv().is_ok() {}
        thread::sleep(Duration::from_millis(50));
        assert!(rx.try_recv().is_err(), "a paused pump should be silent");
        drop(audio);
    }

    #[test]
    fn a_paused_seek_sends_nothing_until_playback_resumes() {
        let (tx, rx) = channel();
        let mut audio = RetroWaveAudio::new(device_with(tx), two_writes_apart(50));
        thread::sleep(Duration::from_millis(30));
        while rx.try_recv().is_ok() {}

        audio.seek_pos(2);
        thread::sleep(Duration::from_millis(50));
        assert!(
            rx.try_recv().is_err(),
            "seeking while paused must not disturb the chip"
        );

        audio.play();
        thread::sleep(Duration::from_millis(50));
        assert!(rx.try_recv().is_ok(), "resuming should reconcile the chip");
    }

    /// The point of the whole wall-clock design: a delay in the song becomes the
    /// same delay on the wire.
    #[test]
    fn playing_spaces_the_writes_by_the_songs_own_delay() {
        let (tx, rx) = channel();
        let mut audio = RetroWaveAudio::new(device_with(tx), two_writes_apart(100));
        let _ = drain_burst(&rx, Duration::from_millis(30));

        audio.play();
        // The reconcile burst and the song's first write arrive together.
        let (_, first_end) = drain_burst(&rx, Duration::from_millis(40));
        let first_end = first_end.expect("playing should write something");

        // The next write is on the far side of the song's 100 ms delay.
        let (_, at) = drain_burst(&rx, Duration::from_secs(2));
        let gap = at.expect("the post-delay write").duration_since(first_end);
        assert!(
            (Duration::from_millis(60)..=Duration::from_millis(160)).contains(&gap),
            "expected roughly a 100 ms gap, got {gap:?}"
        );
        drop(audio);
    }

    #[test]
    fn pausing_releases_the_sounding_note_and_resuming_restores_it() {
        let (tx, rx) = channel();
        let mut audio = RetroWaveAudio::new(device_with(tx), held_note(250));
        let _ = drain_burst(&rx, Duration::from_millis(30));

        audio.play();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(40));
        assert!(
            crate::protocol::decode_writes(&bytes).contains(&(crate::Bank::Zero, 0xB0, 0x31)),
            "the song's key-on should reach the chip"
        );

        audio.pause();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(40));
        let writes = crate::protocol::decode_writes(&bytes);
        assert!(
            writes.contains(&(crate::Bank::Zero, 0xB0, 0x11)),
            "pausing should key the note off, got {writes:02X?}"
        );

        audio.play();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(40));
        let writes = crate::protocol::decode_writes(&bytes);
        assert!(
            writes.contains(&(crate::Bank::Zero, 0xB0, 0x31)),
            "resuming should key it back on, got {writes:02X?}"
        );
        drop(audio);
    }

    /// A paused chip must stay silent however much the user scrubs.
    #[test]
    fn scrubbing_while_paused_never_restarts_the_note() {
        let (tx, rx) = channel();
        let mut audio = RetroWaveAudio::new(device_with(tx), held_note(250));
        audio.play();
        let _ = drain_burst(&rx, Duration::from_millis(40));
        audio.pause();
        let _ = drain_burst(&rx, Duration::from_millis(40));

        for pos in [0, 1, 2, 1, 0] {
            audio.seek_pos(pos);
        }
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(60));
        assert!(
            bytes.is_empty(),
            "paused seeks must not reach the device: {:02X?}",
            crate::protocol::decode_writes(&bytes)
        );
        drop(audio);
    }

    #[test]
    fn dropping_the_player_silences_the_chip_and_returns_the_device() {
        let (tx, rx) = channel();
        let mut audio = RetroWaveAudio::new(device_with(tx), two_writes_apart(50));
        audio.play();
        thread::sleep(Duration::from_millis(40));
        while rx.try_recv().is_ok() {}

        let device = audio.into_device();
        assert!(
            device.is_some(),
            "the port should come back for the next song"
        );
        let mut bytes = Vec::new();
        while let Ok((_, chunk)) = rx.try_recv() {
            bytes.extend_from_slice(&chunk);
        }
        assert!(
            !bytes.is_empty(),
            "shutting down should sweep the chip silent"
        );
    }

    #[test]
    fn a_dead_port_stops_the_pump_and_reports_it() {
        #[derive(Debug)]
        struct DiesOnThirdWrite(u32);

        impl SerialIo for DiesOnThirdWrite {
            fn write_all(&mut self, _bytes: &[u8]) -> Result<(), io::Error> {
                self.0 += 1;
                if self.0 > 3 {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "unplugged"));
                }
                Ok(())
            }
            fn flush(&mut self) -> Result<(), io::Error> {
                Ok(())
            }
        }

        let device = Device::with_io(Box::new(DiesOnThirdWrite(0)), "DYING".to_owned())
            .expect("bring-up writes fewer than three times");
        let mut audio = RetroWaveAudio::new(device, two_writes_apart(50));
        audio.play();
        thread::sleep(Duration::from_millis(120));

        assert!(
            audio.is_finished(),
            "a stopped pump must not look like it is still playing"
        );
        assert!(
            audio.take_error().is_some(),
            "the failure should be reported"
        );
    }
}
