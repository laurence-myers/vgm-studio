//! Playback on a wall clock.
//!
//! The emulated backend is paced by the sound card asking for samples. Hardware
//! output has no such pull -- the audio never passes through this program -- so a
//! thread walks a [`VgmEngine`] against [`Instant`] instead, handing the register
//! writes it produces to the device as they fall due.
//!
//! The engine is the same one every other chip plays through: it hosts the board's
//! [`SerialOpl3Chip`] as an [`OplCoreAdapter`](vgms_synth::OplCoreAdapter) (Stage K
//! / ou-1), so the OPL family is no longer a separate `PlayerEngine` path -- an OPL
//! `DroSong` is projected to a VGM and driven here exactly as a native OPL VGM is. The
//! samples the engine renders are discarded; the register writes it makes to the
//! shared chip are the point, and the shadow/`hw` model in [`SerialOpl3Chip`] turns
//! them into the minimal wire traffic the board needs.

use std::{
    cell::Cell,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use vgms_core::{OplType, VgmFile};
use vgms_synth::{
    ChipMuting, ChipPanning, LoopConfig, Muting, NATIVE_SAMPLE_RATE, Panning, Position, VgmEngine,
    opl::OplChip, opl_chip_muting, opl_chip_panning, opl_hardware_core,
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

/// The board's chip, shared between the engine's adapter(s) and the pump.
///
/// The pump drives one YMF262 through a [`VgmEngine`] whose voices write to it,
/// but the pump also reaches the chip directly -- to materialize after a seek,
/// release notes on a pause, sweep it silent, and drain its wire. Rust ownership
/// forces that sharing through an `Arc<Mutex<_>>`; everything runs on the pump
/// thread, so the mutex is never contended and exists only for the shared hold.
type SharedChip = Arc<Mutex<SerialOpl3Chip>>;

/// Locks the shared chip, recovering a poisoned mutex.
///
/// Poisoning means the pump panicked while a write held the lock; the outer
/// `catch_unwind` is already handling that, the chip's register bytes are still
/// coherent, and the shutdown path still owes the board a silencing sweep -- so
/// taking the guard back beats leaving the chip sounding.
fn lock_chip(chip: &SharedChip) -> MutexGuard<'_, SerialOpl3Chip> {
    chip.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The [`OplChip`] an engine voice writes through: it forwards every write to the
/// one shared [`SerialOpl3Chip`], shifting a dual-OPL2's second instance onto the
/// YMF262's high bank.
///
/// A real dual-OPL2 board is one OPL3 with the second OPL2 mapped to bank 1, but
/// the (projected or native) VGM declares two YM3812 instances that each address
/// registers `0x0nn`. The second instance's wrapper ORs `0x100` in, so the two
/// banks stay distinct on the one chip -- the same split the DRO projection
/// encodes. The first instance (and a single OPL2/OPL3) shifts nothing: an OPL3's
/// own high-bank writes already carry their bank in the adapter's port.
#[derive(Debug, Clone)]
struct SharedOplChip {
    chip: SharedChip,
    bank_shift: u16,
}

impl OplChip for SharedOplChip {
    fn reset(&mut self, sample_rate: u32) {
        lock_chip(&self.chip).reset(sample_rate);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        lock_chip(&self.chip).write_reg(reg | self.bank_shift, value);
    }

    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        lock_chip(&self.chip).write_reg_buffered(reg | self.bank_shift, value);
    }

    /// Silence: the sound comes out of the board, not this program.
    fn generate_samples(&mut self, buffer: &mut [i16]) {
        buffer.fill(0);
    }
}

/// Builds the engine that drives `chip` for `file`, at the OPL's native rate.
///
/// Every OPL voice the header declares gets an [`opl_hardware_core`] over a
/// [`SharedOplChip`]: the first on the low bank, a dual-OPL2's second on the
/// high. Running at [`NATIVE_SAMPLE_RATE`] makes the engine's resampler an
/// identity pass, so the writes land on the same frame boundaries a
/// `PlayerEngine` put them on.
fn opl_engine(file: Arc<VgmFile>, chip: &SharedChip) -> VgmEngine {
    let chip = Arc::clone(chip);
    // Which OPL voice we are building, so a dual-OPL2's second instance takes the
    // high bank. `with_cores` calls the factory in instance order, so the count
    // is the instance for the one OPL chip a hardware song can hold.
    let opl_voices = Cell::new(0u16);
    VgmEngine::with_cores(file, NATIVE_SAMPLE_RATE, move |kind| {
        if !vgms_synth::registry::OPL_CHIPS.contains(&kind) {
            // A hardware song is wholly OPL, so this is a safety net; a non-OPL
            // chip simply gets no voice (silence) rather than an OPL adapter.
            return None;
        }
        let voice = opl_voices.get();
        opl_voices.set(voice + 1);
        let bank_shift = if voice == 0 { 0 } else { 0x100 };
        let shared = SharedOplChip {
            chip: Arc::clone(&chip),
            bank_shift,
        };
        Some(opl_hardware_core(Box::new(shared), kind))
    })
}

/// Control messages for the pump thread.
#[derive(Debug, Clone)]
enum Command {
    Play,
    Pause,
    SeekMs(u32),
    SeekPos(usize),
    Rewind,
    SetMuting(Muting),
    SetChipMuting(ChipMuting),
    SetPanning(Panning),
    SetChipPanning(ChipPanning),
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
    /// Starts a pump for `file` on `device`, paused.
    ///
    /// `file` is an OPL VGM -- a native one, or the projection of an OPL `DroSong`
    /// the service made. `opl` is `Some(type)` when the document is a DRO, whose
    /// OPL panel speaks the [`Muting`]/[`Panning`] vocabulary the pump must
    /// translate; `None` when it is an OPL VGM, which the generic per-chip mixer
    /// drives in [`ChipMuting`]/[`ChipPanning`] directly. It matches the native
    /// backend's `Engine::opl`.
    ///
    /// Takes ownership of the device for as long as the song is loaded; get it
    /// back with [`Self::into_device`] rather than reopening the port, which
    /// costs a chip reset and can fail outright on a port only just closed.
    #[must_use]
    pub fn new(device: Device, file: Arc<VgmFile>, opl: Option<OplType>) -> Self {
        let (commands, consumer) = rtrb::RingBuffer::new(64);
        let shared = Arc::new(SharedState::default());
        let stop = Arc::new(AtomicBool::new(false));

        let pump = thread::Builder::new()
            .name("retrowave-pump".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                move || run_pump(device, file, opl, consumer, shared, stop)
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

    /// Replaces the channel/percussion muting (an OPL document's vocabulary).
    pub fn set_muting(&mut self, muting: Muting) {
        self.send(Command::SetMuting(muting));
    }

    /// Replaces the any-chip channel mutes (an OPL VGM's vocabulary).
    pub fn set_chip_muting(&mut self, muting: ChipMuting) {
        self.send(Command::SetChipMuting(muting));
    }

    /// Replaces the per-channel panning (an OPL document's vocabulary).
    ///
    /// Only its effect on the song's own writes reaches the hardware: the
    /// emulator's panpot registers do not exist on a YMF262.
    pub fn set_panning(&mut self, panning: Panning) {
        self.send(Command::SetPanning(panning));
    }

    /// Replaces the any-chip panning (an OPL VGM's vocabulary).
    pub fn set_chip_panning(&mut self, panning: ChipPanning) {
        self.send(Command::SetChipPanning(panning));
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
    file: Arc<VgmFile>,
    opl: Option<OplType>,
    consumer: rtrb::Consumer<Command>,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
) -> Option<Device> {
    // The chip's own OPL type (for its OPL2-on-OPL3 wire fix-ups) comes from the
    // file, whichever arm the source was; the vocabulary routing (`opl`) is the
    // source-arm distinction the service passes in.
    let chip_type = file.opl().map_or(OplType::Opl3, |opl| opl.opl_type());
    let chip: SharedChip = Arc::new(Mutex::new(SerialOpl3Chip::new(chip_type)));
    let mut engine = opl_engine(Arc::clone(&file), &chip);

    // A panic in here would otherwise leave the chip holding whatever it was
    // playing -- a real one does not stop when the program does.
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        pump_loop(
            &mut device,
            &mut engine,
            &chip,
            opl,
            consumer,
            &shared,
            &stop,
        )
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

    lock_chip(&chip).mute_sweep();
    // Best effort: if this fails the port is already gone, and a hard reset
    // belongs to closing the device rather than unloading a song.
    let _ = flush(&mut device, &chip);
    Some(device)
}

fn pump_loop(
    device: &mut Device,
    engine: &mut VgmEngine,
    chip: &SharedChip,
    opl: Option<OplType>,
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
                        lock_chip(chip).release_all_notes();
                    }
                }
                Command::SeekMs(ms) => {
                    engine.seek_to_ms(ms);
                    reconcile = true;
                }
                Command::SeekPos(pos) => {
                    engine.seek_to_row(pos);
                    reconcile = true;
                }
                Command::Rewind => {
                    engine.rewind();
                    reconcile = true;
                }
                // Route the two vocabularies the way the native backend does: an
                // OPL document (`opl` set) translates its panel's Muting/Panning
                // to the generic engine's, an OPL VGM speaks the generic one and
                // ignores the OPL panel's -- and vice versa. A muted channel's
                // key-off reaches the wire through the gate on the next flush, so
                // muting needs no reconcile; a pan change re-emits through the
                // shadow, so it does.
                Command::SetMuting(muting) => {
                    if let Some(opl_type) = opl {
                        engine.set_muting(opl_chip_muting(&muting, opl_type));
                    }
                }
                Command::SetChipMuting(muting) => {
                    if opl.is_none() {
                        engine.set_muting(muting);
                    }
                }
                Command::SetPanning(panning) => {
                    if let Some(opl_type) = opl {
                        engine.set_panning(opl_chip_panning(&panning, opl_type));
                        reconcile = true;
                    }
                }
                Command::SetChipPanning(panning) => {
                    if opl.is_none() {
                        engine.set_panning(panning);
                        reconcile = true;
                    }
                }
                Command::SetLoop(config) => engine.set_loop(config),
            }
        }

        // Reconciling while paused would undo the pause: the engine's seek
        // replays key-on bits into the shadow, and materialize would sound them
        // on a real chip. The shadow keeps the intent; resuming plays it out.
        if reconcile && playing {
            lock_chip(chip).materialize();
        }

        // Unconditional: writes made while paused -- released notes, a channel
        // just muted -- have to reach the device too.
        flush(device, chip)?;

        if playing && !was_finished {
            // The samples are discarded; walking the stream is what makes the
            // engine's voices write to the shared chip.
            engine.render(&mut scratch);
            flush(device, chip)?;

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
            lock_chip(chip).mute_sweep();
            flush(device, chip)?;
        }
        was_finished = finished;
        shared.finished.store(finished, Ordering::Relaxed);

        deadline += QUANTUM;
        let now = Instant::now();
        if let Some(remaining) = deadline.checked_duration_since(now) {
            // Absolute deadlines, so a slow pass is made up by the next one
            // rather than accumulating into drift. sleep is sub-millisecond
            // accurate on Windows, finer than the quantum -- no spin-waiting.
            thread::sleep(remaining);
        } else if now.duration_since(deadline) > MAX_LAG {
            deadline = now;
        }
    }

    Ok(())
}

/// Hands whatever the chip has queued to the device.
fn flush(device: &mut Device, chip: &SharedChip) -> Result<(), crate::device::Error> {
    let mut chip = lock_chip(chip);
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
    use std::{
        io,
        sync::mpsc::{Sender, channel},
    };
    use vgms_core::vgm::ChipKind;
    use vgms_core::{DroDataV2, DroSong, OplType};

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

    /// Projects a DRO fixture the way the service does, tagging it as an OPL
    /// document (the `Some(opl_type)` vocabulary the pump translates).
    fn dro(song: DroSong) -> (Arc<VgmFile>, Option<OplType>) {
        let opl_type = song.opl_type;
        let file = vgms_core::convert::opl_song_to_vgm_file(&song).expect("a DRO projects");
        (Arc::new(file), Some(opl_type))
    }

    /// A song that writes a register, waits `delay_ms` (at most 256), then
    /// writes another.
    fn two_writes_apart(delay_ms: u32) -> (Arc<VgmFile>, Option<OplType>) {
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
        dro(DroSong::dro_v2(
            "test".to_owned(),
            DroDataV2::new(data, vec![0x20, 0x40], SHORT_DELAY, LONG_DELAY)
                .expect("a well-formed fixture"),
            delay_ms,
            OplType::Opl3,
        ))
    }

    /// A song that keys a note on and holds it for `hold_ms` (at most 256).
    fn held_note(hold_ms: u32) -> (Arc<VgmFile>, Option<OplType>) {
        const SHORT_DELAY: u8 = 0xFE;
        const LONG_DELAY: u8 = 0xFF;
        let data = vec![
            0x00,
            0x31, // codemap[0], so register 0xB0 = key on, block 4
            SHORT_DELAY,
            (hold_ms - 1) as u8,
        ];
        dro(DroSong::dro_v2(
            "held.dro".to_owned(),
            DroDataV2::new(data, vec![0xB0, 0x20], SHORT_DELAY, LONG_DELAY)
                .expect("a well-formed fixture"),
            hold_ms,
            OplType::Opl3,
        ))
    }

    /// Writes a `u32` little-endian into a VGM byte buffer.
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A native OPL VGM (not a projection) with `body` as its command stream and
    /// `clock` on the YM3812 slot -- the `None` (generic-vocabulary) arm.
    fn opl_vgm(clock: u32, body: &[u8]) -> (Arc<VgmFile>, Option<OplType>) {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x151);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        put_u32(&mut bytes, ChipKind::Ym3812.clock_offset(), clock);
        bytes.extend_from_slice(body);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        let file = vgms_core::vgm::file::read("opl.vgm", &bytes).expect("a walkable OPL VGM");
        assert!(file.is_opl(), "the fixture is an OPL VGM");
        (Arc::new(file), None)
    }

    fn device_with(events: Sender<(Instant, Vec<u8>)>) -> Device {
        Device::with_io(Box::new(TimedIo { events })).expect("bring-up")
    }

    fn audio(device: Device, fixture: (Arc<VgmFile>, Option<OplType>)) -> RetroWaveAudio {
        RetroWaveAudio::new(device, fixture.0, fixture.1)
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
        let audio = audio(device, two_writes_apart(50));
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
        let mut audio = audio(device_with(tx), two_writes_apart(50));
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
        let mut audio = audio(device_with(tx), two_writes_apart(100));
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
        let mut audio = audio(device_with(tx), held_note(250));
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

    /// Mute everything, seek into the song, then play: no key-on may reach the
    /// real chip while every channel is muted. A muted seek replays key-ons that
    /// nothing keys off, so materialize could otherwise ring them on the hardware
    /// -- the gate clears the key bit on the replay, so the shadow (and the wire)
    /// stay silent.
    #[test]
    fn playing_a_selection_with_everything_muted_stays_silent() {
        let (tx, rx) = channel();
        let mut audio = audio(device_with(tx), held_note(100));
        audio.set_muting(Muting::silent());
        let _ = drain_burst(&rx, Duration::from_millis(40));

        audio.seek_pos(1); // past the key-on, exactly like play-from-selection
        audio.play();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(60));
        let writes = crate::protocol::decode_writes(&bytes);

        assert!(!writes.is_empty(), "playing must still reconcile the chip");
        assert!(
            !writes
                .iter()
                .any(|&(_, reg, value)| (0xB0..=0xB8).contains(&reg) && value & 0x20 != 0),
            "no key-on may reach the hardware while every channel is muted: {writes:02X?}"
        );
        assert!(
            !writes
                .iter()
                .any(|&(_, reg, value)| reg == 0xBD && value & 0x1F != 0),
            "no percussion key may reach the hardware either: {writes:02X?}"
        );
        drop(audio);
    }

    /// A paused chip must stay silent however much the user scrubs.
    #[test]
    fn scrubbing_while_paused_never_restarts_the_note() {
        let (tx, rx) = channel();
        let mut audio = audio(device_with(tx), held_note(250));
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
        let mut audio = audio(device_with(tx), two_writes_apart(50));
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

        let device = Device::with_io(Box::new(DiesOnThirdWrite(0)))
            .expect("bring-up writes fewer than three times");
        let mut audio = audio(device, two_writes_apart(50));
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

    /// A native OPL VGM (the `None` arm) plays its own command stream to the
    /// chip, exactly as a projected DRO does.
    #[test]
    fn an_opl_vgm_plays_its_writes_to_the_chip() {
        let (tx, rx) = channel();
        // A YM3812 key-on, then a wait, then end.
        let mut audio = audio(
            device_with(tx),
            opl_vgm(3_579_545, &[0x5A, 0xB0, 0x31, 0x61, 0x00, 0x80, 0x66]),
        );
        let _ = drain_burst(&rx, Duration::from_millis(30));

        audio.play();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(40));
        let writes = crate::protocol::decode_writes(&bytes);
        assert!(
            writes.contains(&(crate::Bank::Zero, 0xB0, 0x31)),
            "the OPL VGM's key-on should reach the chip, got {writes:02X?}"
        );
        drop(audio);
    }

    /// A dual-OPL2 VGM is two YM3812 instances on one physical YMF262: the first
    /// chip's writes land on bank 0, the second's (opcode `0xAA`) on bank 1. This
    /// is the routing the single shared chip's two adapters have to get right.
    #[test]
    fn a_dual_opl2_vgm_routes_its_second_chip_to_the_high_bank() {
        const DUAL_STEREO: u32 = 3_579_545 | 0xC000_0000; // dual (bit 30) + SB Pro (bit 31)
        let (tx, rx) = channel();
        // Instance 0 (0x5A) keys channel 0; instance 1 (0xAA) keys channel 1.
        let mut audio = audio(
            device_with(tx),
            opl_vgm(
                DUAL_STEREO,
                &[0x5A, 0xB0, 0x31, 0xAA, 0xB1, 0x31, 0x61, 0x00, 0x80, 0x66],
            ),
        );
        let _ = drain_burst(&rx, Duration::from_millis(30));

        audio.play();
        let (bytes, _) = drain_burst(&rx, Duration::from_millis(40));
        let writes = crate::protocol::decode_writes(&bytes);
        assert!(
            writes.contains(&(crate::Bank::Zero, 0xB0, 0x31)),
            "the first chip's key-on is on bank 0, got {writes:02X?}"
        );
        assert!(
            writes.contains(&(crate::Bank::One, 0xB1, 0x31)),
            "the second chip's key-on is on bank 1, got {writes:02X?}"
        );
        drop(audio);
    }
}
