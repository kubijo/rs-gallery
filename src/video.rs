//! Recordings: a run of frames written as an animated PNG, or as an MP4.
//!
//! Frame *n* shows at exactly *n* / fps, whatever the format,
//! so how fast a frame renders or encodes cannot move it.

use std::{
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use image::RgbaImage;

/// How a recording is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VideoFormat {
    /// Lossless and byte-identical between runs,
    /// so a recording can itself be compared.
    Apng,
    /// H.264: small, lossy, and seekable in any player.
    #[default]
    Mp4,
}

impl VideoFormat {
    const ALL: [Self; 2] = [Self::Apng, Self::Mp4];

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Apng => "apng",
            Self::Mp4 => "mp4",
        }
    }

    /// The file's writer, started with the first frame.
    fn start(
        self,
        file: File,
        first: RgbaImage,
        fps: u16,
        grows: bool,
    ) -> Result<Box<dyn Stream>, String> {
        match self {
            Self::Apng => Ok(Box::new(Apng::start(file, first, fps, grows)?)),
            Self::Mp4 => Ok(Box::new(Mp4::start(file, first, fps)?)),
        }
    }
}

impl std::str::FromStr for VideoFormat {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        match raw {
            "apng" => Ok(Self::Apng),
            "mp4" => Ok(Self::Mp4),
            _ => Err(format!(
                "`{raw}` is not a recording format: `apng` or `mp4`"
            )),
        }
    }
}

/// The formats a shot records in, each once, written from the same frames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VideoFormats(u8);

impl VideoFormats {
    fn bit(format: VideoFormat) -> u8 {
        1 << (format as u8)
    }

    /// `formats`, unless it names none.
    pub(crate) fn of(formats: impl IntoIterator<Item = VideoFormat>) -> Option<Self> {
        let set = formats
            .into_iter()
            .fold(0, |set, format| set | Self::bit(format));
        (set != 0).then_some(Self(set))
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = VideoFormat> {
        VideoFormat::ALL
            .into_iter()
            .filter(move |&format| self.0 & Self::bit(format) != 0)
    }
}

impl Default for VideoFormats {
    fn default() -> Self {
        Self(Self::bit(VideoFormat::default()))
    }
}

impl From<VideoFormat> for VideoFormats {
    fn from(format: VideoFormat) -> Self {
        Self(Self::bit(format))
    }
}

/// `apng`, or several joined by commas: `apng,mp4`.
impl std::str::FromStr for VideoFormats {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        let formats = raw
            .split(',')
            .map(|one| one.trim().parse())
            .collect::<Result<Vec<VideoFormat>, _>>()?;
        Self::of(formats).ok_or_else(|| "no recording format given".to_owned())
    }
}

/// One format, or a list of them: `"mp4"`, `["apng", "mp4"]`.
impl<'de> serde::Deserialize<'de> for VideoFormats {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Strings parsed here rather than variants tried by serde,
        // whose untagged miss would not say which formats there are.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        let names = match OneOrMany::deserialize(deserializer)? {
            OneOrMany::One(name) => vec![name],
            OneOrMany::Many(names) => names,
        };
        let formats = names
            .iter()
            .map(|name| name.parse())
            .collect::<Result<Vec<VideoFormat>, _>>()
            .map_err(serde::de::Error::custom)?;
        Self::of(formats).ok_or_else(|| {
            serde::de::Error::custom("an empty list of recording formats records nothing")
        })
    }
}

/// What the encoder thread is handed.
enum Message {
    Frame(RgbaImage),
    /// Show the last frame for this many frames more.
    Hold(u32),
    /// Close the file.
    Finish,
}

/// A recording's file as frames reach it, on the encoder thread.
trait Stream {
    fn push(&mut self, image: RgbaImage) -> Result<(), String>;

    fn hold(&mut self, frames: u32) -> Result<(), String>;

    /// Close the file, returning how many frames it holds.
    fn finish(self: Box<Self>) -> Result<u32, String>;
}

/// What a finished recording came to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Encoded {
    /// Frames handed in.
    pub(crate) frames: u32,
    /// Frames added by holding the last one, beyond `frames`.
    pub(crate) held: u32,
    /// Frames the file holds, a repeat having lengthened
    /// the frame before it instead.
    pub(crate) written: u32,
    /// Time the encoder spent on all frames together.
    pub(crate) encode: Duration,
    /// The most frames waiting for the encoder at once.
    pub(crate) deepest_queue: usize,
}

/// Frames a recording may queue ahead of the encoder,
/// which bounds its memory to a few frames
/// while rendering and encoding overlap.
const QUEUE: usize = 2;

/// The most recorded frames one written frame
/// can stand for: a delay's numerator is a `u16`.
const MAX_DELAY: u32 = u16::MAX as u32;

/// A recording, its frames encoded on a thread of its own.
pub(crate) struct Recorder {
    frames: Option<mpsc::SyncSender<Message>>,
    worker: Option<thread::JoinHandle<Result<Encoded, String>>>,
    queued: Arc<AtomicUsize>,
    deepest_queue: usize,
    count: u32,
    held: u32,
    /// Why the encoder stopped, once that is known.
    failed: Option<String>,
}

impl Recorder {
    /// Start a recording into `path` at `fps`;
    /// the file is created now, so a bad path fails first.
    ///
    /// # Errors
    /// When `path` cannot be created or `fps` is zero.
    pub(crate) fn create(path: &Utf8Path, fps: u16, format: VideoFormat) -> Result<Self, String> {
        Self::open(path, fps, format, false)
    }

    /// [`create`](Self::create) an APNG as large as its largest frames, of no use
    /// before it is [finished](Self::finish).
    ///
    /// # Errors
    /// As [`create`](Self::create).
    pub(crate) fn growing(path: &Utf8Path, fps: u16) -> Result<Self, String> {
        Self::open(path, fps, VideoFormat::Apng, true)
    }

    fn open(path: &Utf8Path, fps: u16, format: VideoFormat, grows: bool) -> Result<Self, String> {
        if fps == 0 {
            return Err("a recording needs at least one frame per second".to_owned());
        }
        // Readable too: closing it reads back
        // to find the frame count's chunk.
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("create `{path}`: {e}"))?;
        let (frames, incoming) = mpsc::sync_channel(QUEUE);
        let queued = Arc::new(AtomicUsize::new(0));
        let worker = {
            let queued = Arc::clone(&queued);
            let target = path.to_owned();
            thread::Builder::new()
                .name("gallery-recording".to_owned())
                .spawn(move || {
                    let start = |file, first| format.start(file, first, fps, grows);
                    encode(file, start, !grows, &incoming, &queued)
                        .map_err(|e| format!("write `{target}`: {e}"))
                })
                .map_err(|e| format!("start the encoder for `{path}`: {e}"))?
        };
        Ok(Self {
            frames: Some(frames),
            worker: Some(worker),
            queued,
            deepest_queue: 0,
            count: 0,
            held: 0,
            failed: None,
        })
    }

    /// The encoder's own error, once it has stopped taking frames.
    fn failure(&mut self) -> String {
        self.frames = None;
        let stopped = match self.worker.take().map(thread::JoinHandle::join) {
            Some(Ok(Err(error))) => error,
            Some(Err(_)) => "the encoder panicked".to_owned(),
            Some(Ok(Ok(_))) | None => "the encoder stopped early".to_owned(),
        };
        self.failed.get_or_insert(stopped).clone()
    }

    /// Hand over the next frame: the same size as the last,
    /// except in one that is [`growing`](Self::growing).
    ///
    /// # Errors
    /// When the encoder has already failed;
    /// the error is its reason.
    pub(crate) fn frame(&mut self, image: RgbaImage) -> Result<(), String> {
        self.send(Message::Frame(image))?;
        self.count += 1;
        Ok(())
    }

    /// Show the last frame for `frames` frames more, drawing nothing new.
    ///
    /// # Errors
    /// When the encoder has already failed;
    /// the error is its reason.
    pub(crate) fn hold(&mut self, frames: u32) -> Result<(), String> {
        if frames == 0 {
            return Ok(());
        }
        self.send(Message::Hold(frames))?;
        self.held += frames;
        Ok(())
    }

    fn send(&mut self, message: Message) -> Result<(), String> {
        let Some(frames) = &self.frames else {
            return Err(self.failure());
        };
        let waiting = self.queued.fetch_add(1, Ordering::Relaxed) + 1;
        self.deepest_queue = self.deepest_queue.max(waiting);
        if frames.send(message).is_err() {
            return Err(self.failure());
        }
        Ok(())
    }

    /// Write out what is pending and close the file.
    ///
    /// # Errors
    /// When nothing was recorded, or the file could not be written.
    pub(crate) fn finish(mut self) -> Result<Encoded, String> {
        if let Some(frames) = self.frames.take() {
            // An encoder that stopped taking messages says why once joined.
            _ = frames.send(Message::Finish);
        }
        let encoded = match self.worker.take().map(thread::JoinHandle::join) {
            Some(Ok(result)) => result?,
            Some(Err(_)) => return Err("the encoder panicked".to_owned()),
            None => return Err(self.failure()),
        };
        Ok(Encoded {
            frames: self.count,
            held: self.held,
            deepest_queue: self.deepest_queue,
            ..encoded
        })
    }
}

/// A recording given up leaves no thread behind that still has its file open.
impl Drop for Recorder {
    fn drop(&mut self) {
        self.frames = None;
        if let Some(worker) = self.worker.take() {
            _ = worker.join();
        }
    }
}

/// The encoder thread: every frame in, one file out. A recording dropped before it is finished
/// is closed all the same when it is `wanted`, for what it holds until then.
fn encode(
    file: File,
    start: impl FnOnce(File, RgbaImage) -> Result<Box<dyn Stream>, String>,
    wanted: bool,
    incoming: &mpsc::Receiver<Message>,
    queued: &AtomicUsize,
) -> Result<Encoded, String> {
    let first = match incoming.recv() {
        Ok(Message::Frame(first)) => first,
        Ok(Message::Hold(_)) => return Err("a hold before any frame to hold".to_owned()),
        Ok(Message::Finish) | Err(_) => {
            return Err("a recording needs at least one frame".to_owned());
        }
    };
    queued.fetch_sub(1, Ordering::Relaxed);
    let mut started = Instant::now();
    let mut stream = start(file, first)?;
    let mut spent = started.elapsed();
    loop {
        let message = match incoming.recv() {
            Ok(message) => message,
            Err(_) if wanted => Message::Finish,
            Err(_) => return Err("the recording was given up".to_owned()),
        };
        if !matches!(message, Message::Finish) {
            queued.fetch_sub(1, Ordering::Relaxed);
        }
        started = Instant::now();
        match message {
            Message::Frame(image) => stream.push(image)?,
            Message::Hold(frames) => stream.hold(frames)?,
            Message::Finish => {
                let written = stream.finish()?;
                return Ok(Encoded {
                    frames: 0,
                    held: 0,
                    written,
                    encode: spent + started.elapsed(),
                    deepest_queue: 0,
                });
            }
        }
        spent += started.elapsed();
    }
}

/// A frame waiting to learn how long it shows,
/// which only the next different frame tells.
struct Pending {
    x: u32,
    y: u32,
    pixels: RgbaImage,
    /// How many recorded frames it stands for.
    repeats: u32,
}

/// The APNG as it is written: the canvas
/// a viewer shows, and the frame not yet written.
struct Apng {
    writer: png::Writer<BufWriter<File>>,
    /// The same file, to patch the frame count
    /// into once the writer has closed it.
    file: File,
    fps: u16,
    /// The full frame the viewer shows once `pending` is written.
    shown: RgbaImage,
    pending: Pending,
    written: u32,
    /// The first frame's size, and its pixels in one that may grow.
    start: (u32, u32),
    first: Option<RgbaImage>,
    /// The delay the first frame was written with.
    first_shows: u16,
}

/// A header's width and height until the last frame
/// tells the real ones, past any a canvas can have.
const UNKNOWN_SIZE: u32 = 1 << 20;

/// An animated PNG's header, its frame count and size to be known later.
fn apng_header<W: Write>(sink: W, size: (u32, u32), frames: u32) -> Result<png::Writer<W>, String> {
    let mut encoder = png::Encoder::new(sink, size.0, size.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_animated(frames, 0).map_err(|e| e.to_string())?;
    encoder.write_header().map_err(|e| e.to_string())
}

impl Apng {
    fn start(file: File, first: RgbaImage, fps: u16, grows: bool) -> Result<Self, String> {
        let patch = file.try_clone().map_err(|e| e.to_string())?;
        // Both are unknown until the stream ends, and come before any frame:
        // placeholders now, patched by `set_header` once the file is closed.
        let writer = apng_header(BufWriter::new(file), (UNKNOWN_SIZE, UNKNOWN_SIZE), u32::MAX)?;
        Ok(Self {
            writer,
            file: patch,
            fps,
            start: first.dimensions(),
            first: grows.then(|| first.clone()),
            first_shows: 0,
            pending: Pending {
                x: 0,
                y: 0,
                pixels: first.clone(),
                repeats: 1,
            },
            shown: first,
            written: 0,
        })
    }
}

impl Stream for Apng {
    fn push(&mut self, image: RgbaImage) -> Result<(), String> {
        let mut image = image;
        if image.dimensions() != self.shown.dimensions() {
            let (was, now) = (self.shown.dimensions(), image.dimensions());
            if self.first.is_none() {
                return Err(resized(was, now));
            }
            let size = (was.0.max(now.0), was.1.max(now.1));
            if was != size {
                self.shown = grown(&self.shown, size);
            }
            if now != size {
                image = grown(&image, size);
            }
        }
        let Some(changed) = changed(&self.shown, &image) else {
            self.pending.repeats += 1;
            return Ok(());
        };
        self.write_pending()?;
        self.pending = Pending {
            x: changed.x,
            y: changed.y,
            pixels: image::imageops::crop_imm(
                &image,
                changed.x,
                changed.y,
                changed.width,
                changed.height,
            )
            .to_image(),
            repeats: 1,
        };
        self.shown = image;
        Ok(())
    }

    fn hold(&mut self, frames: u32) -> Result<(), String> {
        self.pending.repeats += frames;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<u32, String> {
        let mut apng = *self;
        apng.write_pending()?;
        apng.writer.finish().map_err(|e| e.to_string())?;
        let size = apng.shown.dimensions();
        match &apng.first {
            Some(first) if size != apng.start => {
                let delay = (apng.first_shows, apng.fps);
                replace_first(&mut apng.file, &grown(first, size), delay, apng.written)?;
            }
            _ => set_header(&mut apng.file, size, apng.written)?,
        }
        Ok(apng.written)
    }
}

/// Why a recording that does not grow takes no frame of another size.
pub(crate) fn resized(was: (u32, u32), now: (u32, u32)) -> String {
    format!(
        "the canvas went from {}×{} to {}×{} while it was recorded",
        was.0, was.1, now.0, now.1
    )
}

/// `image` in the top left corner of a transparent one of `size`.
fn grown(image: &RgbaImage, size: (u32, u32)) -> RgbaImage {
    let mut grown = RgbaImage::new(size.0, size.1);
    image::imageops::replace(&mut grown, image, 0, 0);
    grown
}

impl Apng {
    /// Write the pending frame for as long as it repeats.
    fn write_pending(&mut self) -> Result<(), String> {
        let pending = std::mem::replace(
            &mut self.pending,
            Pending {
                x: 0,
                y: 0,
                pixels: RgbaImage::new(0, 0),
                repeats: 0,
            },
        );
        let mut left = pending.repeats;
        let mut shows = left.min(MAX_DELAY);
        self.write_frame(pending.x, pending.y, &pending.pixels, shows)?;
        // A delay's numerator is 16 bits, so a longer run goes
        // on as one-pixel frames that change nothing.
        while left > shows {
            left -= shows;
            shows = left.min(MAX_DELAY);
            let unchanged = image::imageops::crop_imm(&self.shown, 0, 0, 1, 1).to_image();
            self.write_frame(0, 0, &unchanged, shows)?;
        }
        Ok(())
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "`shows` is at most `MAX_DELAY`"
    )]
    fn write_frame(
        &mut self,
        x: u32,
        y: u32,
        pixels: &RgbaImage,
        shows: u32,
    ) -> Result<(), String> {
        let writer = &mut self.writer;
        let error = |e: png::EncodingError| e.to_string();
        // Position first: a new size is checked against
        // the old position, and the other way round.
        writer.reset_frame_position().map_err(error)?;
        writer
            .set_frame_dimension(pixels.width(), pixels.height())
            .map_err(error)?;
        writer.set_frame_position(x, y).map_err(error)?;
        writer
            .set_frame_delay(shows as u16, self.fps)
            .map_err(error)?;
        writer.write_image_data(pixels.as_raw()).map_err(error)?;
        if self.written == 0 {
            self.first_shows = shows as u16;
        }
        self.written += 1;
        Ok(())
    }
}

/// An H.264 stream in an MP4, every frame encoded and stamped at `n / fps`.
struct Mp4 {
    encoder: openh264::encoder::Encoder,
    muxer: muxide::api::Muxer<BufWriter<File>>,
    fps: u16,
    size: (u32, u32),
    /// `size` rounded up to even: 4:2:0 chroma covers pixels in pairs.
    coded: (u32, u32),
    /// The last frame pushed, encoded again for as long as it is held.
    last: openh264::formats::YUVBuffer,
    written: u32,
}

impl Mp4 {
    fn start(file: File, first: RgbaImage, fps: u16) -> Result<Self, String> {
        use openh264::encoder::{
            Encoder, EncoderConfig, FrameRate, IntraFramePeriod, QpRange, RateControlMode,
            UsageType, VuiConfig,
        };
        let size = first.dimensions();
        let coded = (size.0 + size.0 % 2, size.1 + size.1 % 2);
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            // Screen content turns these off anyway,
            // warning on stderr unless asked to.
            .adaptive_quantization(false)
            .background_detection(false)
            // A skipped frame would leave a gap in the timeline.
            .skip_frames(false)
            // Holds every frame to the QP range below.
            // With no bitrate to aim for whe quality mode would warn
            // that it cannot meet one without skipping frames.
            .rate_control_mode(RateControlMode::Bufferbased)
            .max_frame_rate(FrameRate::from_hz(f32::from(fps)))
            // A keyframe every second, for seeking.
            .intra_frame_period(IntraFramePeriod::from_num_frames(u32::from(fps)))
            // One quality for every frame, fine enough for text and the stage's checkerboard.
            // A bound of 0 is not "no limit": OpenH264 reads it as unset and puts in 26–35
            // for screen content (`encoder_ext.cpp`, `MIN_SCREEN_QP`), visibly blurry.
            .qp(QpRange::new(18, 18))
            // The crate converts RGB to YUV by BT.601 at limited range; players are told so.
            .vui(VuiConfig::bt601());
        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|e| e.to_string())?;
        let muxer = muxide::api::MuxerBuilder::new(BufWriter::new(file))
            .video(
                muxide::api::VideoCodec::H264,
                coded.0,
                coded.1,
                f64::from(fps),
            )
            .build()
            .map_err(|e| e.to_string())?;
        let mut mp4 = Self {
            encoder,
            muxer,
            fps,
            size,
            coded,
            last: Self::yuv(first, coded),
            written: 0,
        };
        mp4.write_last()?;
        Ok(mp4)
    }

    fn yuv(image: RgbaImage, coded: (u32, u32)) -> openh264::formats::YUVBuffer {
        use openh264::formats::{RgbaSliceU8, YUVBuffer};
        let image = padded(image, coded);
        YUVBuffer::from_rgba8_source(RgbaSliceU8::new(
            image.as_raw(),
            (coded.0 as usize, coded.1 as usize),
        ))
    }

    /// Encode [`Self::last`] as the next frame, stamped at its time.
    fn write_last(&mut self) -> Result<(), String> {
        use openh264::encoder::FrameType;
        let encoded = self.encoder.encode(&self.last).map_err(|e| e.to_string())?;
        let keyframe = matches!(encoded.frame_type(), FrameType::IDR | FrameType::I);
        let data = encoded.to_vec();
        if data.is_empty() {
            return Err(format!("the encoder dropped frame {}", self.written));
        }
        self.muxer
            .write_video(
                f64::from(self.written) / f64::from(self.fps),
                &data,
                keyframe,
            )
            .map_err(|e| e.to_string())?;
        self.written += 1;
        Ok(())
    }
}

impl Stream for Mp4 {
    fn push(&mut self, image: RgbaImage) -> Result<(), String> {
        if image.dimensions() != self.size {
            return Err(resized(self.size, image.dimensions()));
        }
        self.last = Self::yuv(image, self.coded);
        self.write_last()
    }

    fn hold(&mut self, frames: u32) -> Result<(), String> {
        (0..frames).try_for_each(|_| self.write_last())
    }

    fn finish(self: Box<Self>) -> Result<u32, String> {
        let mut mp4 = *self;
        mp4.muxer.finish_in_place().map_err(|e| e.to_string())?;
        // `muxide` 0.5 never flushes its writer, and a `BufWriter` dropped tells no error.
        let mut writer = mp4.muxer.into_writer();
        writer.flush().map_err(|e| e.to_string())?;
        Ok(mp4.written)
    }
}

/// `image` grown to `size` by repeating its last column and row.
fn padded(image: RgbaImage, size: (u32, u32)) -> RgbaImage {
    if image.dimensions() == size {
        return image;
    }
    let (width, height) = image.dimensions();
    RgbaImage::from_fn(size.0, size.1, |x, y| {
        *image.get_pixel(x.min(width - 1), y.min(height - 1))
    })
}

/// Where two frames differ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// The smallest rectangle holding every pixel that differs, or `None` when none does.
fn changed(was: &RgbaImage, now: &RgbaImage) -> Option<Rect> {
    let row = was.width() as usize * 4;
    let rows = || {
        was.as_raw()
            .chunks_exact(row)
            .zip(now.as_raw().chunks_exact(row))
    };
    let top = rows().position(|(a, b)| a != b)?;
    let bottom = rows().rposition(|(a, b)| a != b)?;
    let (mut left, mut right) = (usize::MAX, 0);
    for (a, b) in rows().skip(top).take(bottom - top + 1) {
        let pixels = || a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0);
        if let Some(first) = pixels().position(|(p, q)| p != q) {
            left = left.min(first);
            right = right.max(pixels().rposition(|(p, q)| p != q).unwrap_or(first));
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "within an image's u32 dimensions"
    )]
    Some(Rect {
        x: left as u32,
        y: top as u32,
        width: (right - left + 1) as u32,
        height: (bottom - top + 1) as u32,
    })
}

/// Where a PNG's chunks begin, past its signature.
const CHUNKS: u64 = 8;

/// A chunk's length, name and CRC, which lie around its data.
const AROUND_DATA: u64 = 12;

/// The name and data length of the chunk at `at`,
/// the file left where its data begins.
fn chunk_at(file: &mut File, at: u64) -> Result<([u8; 4], u32), String> {
    file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut head = [0; 8];
    file.read_exact(&mut head).map_err(|e| match e.kind() {
        std::io::ErrorKind::UnexpectedEof => "the file ended before its last chunk".to_owned(),
        _ => e.to_string(),
    })?;
    let [a, b, c, d, name @ ..] = head;
    Ok((name, u32::from_be_bytes([a, b, c, d])))
}

/// Write the real size into `IHDR` and the real frame count into `acTL`,
/// over the encoder's placeholders, with the CRCs to match.
fn set_header(file: &mut File, size: (u32, u32), frames: u32) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut at = CHUNKS;
    loop {
        let (name, length) = chunk_at(file, at)?;
        let start = match &name {
            b"IHDR" => [size.0.to_be_bytes(), size.1.to_be_bytes()].concat(),
            b"acTL" => frames.to_be_bytes().to_vec(),
            b"IDAT" => return Err("no `acTL` chunk before the first frame".to_owned()),
            _ => Vec::new(),
        };
        if !start.is_empty() {
            let mut data = vec![0; length as usize];
            file.read_exact(&mut data).map_err(io)?;
            data[..start.len()].copy_from_slice(&start);
            let mut crc = crc32fast::Hasher::new();
            crc.update(&name);
            crc.update(&data);
            file.seek(SeekFrom::Start(at + 8)).map_err(io)?;
            file.write_all(&data).map_err(io)?;
            file.write_all(&crc.finalize().to_be_bytes()).map_err(io)?;
        }
        if &name == b"acTL" {
            return file.flush().map_err(io);
        }
        at += AROUND_DATA + u64::from(length);
    }
}

/// Write the file again with `first` for its first frame, which in an APNG
/// covers the whole picture; those after it follow as they are.
fn replace_first(
    file: &mut File,
    first: &RgbaImage,
    (shows, fps): (u16, u16),
    frames: u32,
) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let png = |e: png::EncodingError| e.to_string();
    let mut at = CHUNKS;
    let mut seen = false;
    let rest = loop {
        let (name, length) = chunk_at(file, at)?;
        let image = &name == b"IDAT";
        if seen && !image {
            break at;
        }
        seen |= image;
        at += AROUND_DATA + u64::from(length);
    };
    file.seek(SeekFrom::Start(rest)).map_err(io)?;
    let mut after = Vec::new();
    file.read_to_end(&mut after).map_err(io)?;

    let mut head = Vec::new();
    let mut writer = apng_header(&mut head, first.dimensions(), frames)?;
    writer.set_frame_delay(shows, fps).map_err(png)?;
    writer.write_image_data(first.as_raw()).map_err(png)?;
    drop(writer);
    // Its `IEND`, as `after` ends in one.
    head.truncate(head.len() - AROUND_DATA as usize);

    file.seek(SeekFrom::Start(0)).map_err(io)?;
    file.write_all(&head).map_err(io)?;
    file.write_all(&after).map_err(io)?;
    let end = file.stream_position().map_err(io)?;
    file.set_len(end).map_err(io)?;
    file.flush().map_err(io)
}

/// A written frame as a viewer shows it:
/// composited over the ones before.
#[cfg(test)]
pub(crate) struct Shown {
    pub(crate) image: RgbaImage,
    pub(crate) delay: (u16, u16),
}

/// Every frame of an APNG as a viewer shows it, with its delay;
/// and how many there are.
fn each_shown(
    path: &Utf8Path,
    mut show: impl FnMut(&RgbaImage, (u16, u16)) -> Result<(), String>,
) -> Result<u32, String> {
    let file = File::open(path).map_err(|e| format!("open `{path}`: {e}"))?;
    let error = |e: png::DecodingError| format!("read `{path}`: {e}");
    let broken = || format!("`{path}` is no recording");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(error)?;
    let frames = reader
        .info()
        .animation_control
        .ok_or_else(broken)?
        .num_frames;
    let mut canvas = RgbaImage::new(reader.info().width, reader.info().height);
    let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(broken)?];
    for _ in 0..frames {
        let output = reader.next_frame(&mut buffer).map_err(error)?;
        let control = *reader.info().frame_control().ok_or_else(broken)?;
        let pixels = buffer[..output.buffer_size()].to_vec();
        let frame = RgbaImage::from_raw(output.width, output.height, pixels).ok_or_else(broken)?;
        image::imageops::replace(
            &mut canvas,
            &frame,
            i64::from(control.x_offset),
            i64::from(control.y_offset),
        );
        show(&canvas, (control.delay_num, control.delay_den))?;
    }
    Ok(frames)
}

/// Why an MP4 can't hold frames of `size`, if it can't:
/// OpenH264 encodes up to 3840×2160, either way up (`encoder.rs` in openh264 0.9).
pub(crate) fn too_large((width, height): (u32, u32)) -> Option<String> {
    let (long, short) = (width.max(height), width.min(height));
    (long > 3_840 || short > 2_160)
        .then(|| format!("{width}×{height} is past the 3840×2160 a video can be"))
}

/// Write the APNG recording at `apng` into `mp4`, each frame at its time,
/// with what `over` has for it laid on by `lay`.
///
/// # Errors
/// When it can't be read, is too large for H.264, or `lay` fails.
pub(crate) fn to_mp4<T: PartialEq>(
    apng: &Utf8Path,
    mp4: File,
    over: &[T],
    mut lay: impl FnMut(&T, &mut RgbaImage) -> Result<(), String>,
) -> Result<(), String> {
    let mut file = Some(mp4);
    let mut stream: Option<Mp4> = None;
    let mut at = 0;
    each_shown(apng, |image, (shows, fps)| {
        for repeat in 0..shows {
            let now = over.get(at);
            let repeats = repeat > 0 && now == over.get(at - 1);
            at += 1;
            if let Some(stream) = stream.as_mut().filter(|_| repeats) {
                stream.hold(1)?;
                continue;
            }
            let mut image = image.clone();
            if let Some(now) = now {
                lay(now, &mut image)?;
            }
            match (&mut stream, file.take()) {
                (Some(stream), _) => stream.push(image)?,
                (None, Some(file)) => stream = Some(Mp4::start(file, image, fps)?),
                (None, None) => return Err("BUG: an MP4 started twice".to_owned()),
            }
        }
        Ok(())
    })?;
    let stream = stream.ok_or_else(|| format!("`{apng}` has no frame"))?;
    Box::new(stream).finish().map(drop)
}

/// The frame count `acTL` declares,
/// and every frame as a viewer shows it.
#[cfg(test)]
pub(crate) fn decode(path: &Utf8Path) -> (u32, Vec<Shown>) {
    let mut shown = Vec::new();
    let declared = each_shown(path, |image, delay| {
        shown.push(Shown {
            image: image.clone(),
            delay,
        });
        Ok(())
    })
    .expect("a recording");
    (declared, shown)
}

/// An MP4's size and how many frames it holds.
#[cfg(test)]
pub(crate) fn mp4_frames(path: &Utf8Path) -> ((u16, u16), u32) {
    let bytes = std::fs::read(path).expect("the recording");
    let size = bytes.len() as u64;
    let mp4 = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
    let (&id, track) = mp4.tracks().iter().next().expect("a video track");
    let frames = mp4.sample_count(id).expect("samples");
    ((track.width(), track.height()), frames)
}

/// Every frame of an MP4.
#[cfg(test)]
pub(crate) fn decode_mp4(path: &Utf8Path) -> Vec<image::RgbImage> {
    use openh264::formats::YUVSource as _;

    let bytes = std::fs::read(path).expect("the recording");
    let size = bytes.len() as u64;
    let mut mp4 = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
    let (&id, track) = mp4.tracks().iter().next().expect("a video track");
    let start = [0, 0, 0, 1];
    let mut annex_b = [
        &start,
        track.sequence_parameter_set().expect("an SPS"),
        &start,
        track.picture_parameter_set().expect("a PPS"),
    ]
    .concat();
    let mut decoder = openh264::decoder::Decoder::new().expect("a decoder");
    (1..=mp4.sample_count(id).expect("samples"))
        .map(|n| {
            let sample = mp4.read_sample(id, n).expect("a sample").expect("present");
            let mut units = &sample.bytes[..];
            while let Some((length, rest)) = units.split_first_chunk::<4>() {
                let (unit, rest) = rest.split_at(u32::from_be_bytes(*length) as usize);
                annex_b.extend_from_slice(&start);
                annex_b.extend_from_slice(unit);
                units = rest;
            }
            let decoded = decoder
                .decode(&annex_b)
                .expect("the frame decodes")
                .expect("a picture");
            annex_b.clear();
            let (width, height) = decoded.dimensions();
            let mut rgb = vec![0; width * height * 3];
            decoded.write_rgb8(&mut rgb);
            let size = |side: usize| u32::try_from(side).expect("a canvas's side");
            image::RgbImage::from_raw(size(width), size(height), rgb).expect("every pixel")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use camino::Utf8PathBuf;

    /// A fresh path in a directory of this test's own.
    fn scratch(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-video")
            .join(name);
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        dir.join("out.png")
    }

    fn record(name: &str, fps: u16, frames: &[RgbaImage]) -> (Utf8PathBuf, Encoded) {
        let path = scratch(name);
        let mut sink = Recorder::create(&path, fps, VideoFormat::Apng).expect("a recording");
        for frame in frames {
            sink.frame(frame.clone())
                .expect("the encoder takes the frame");
        }
        let encoded = sink.finish().expect("the recording closes");
        (path, encoded)
    }

    fn solid(width: u32, height: u32, shade: u8) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba([shade, shade, shade, 255]))
    }

    /// `base` with one pixel at (`x`, `y`) changed.
    fn dotted(base: &RgbaImage, x: u32, y: u32) -> RgbaImage {
        let mut image = base.clone();
        image.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
        image
    }

    #[test]
    fn each_frame_shows_what_was_recorded_for_exactly_its_time() {
        let base = solid(16, 8, 40);
        let frames = [
            base.clone(),
            dotted(&base, 3, 2),
            dotted(&base, 3, 2),
            dotted(&base, 3, 2),
            dotted(&dotted(&base, 3, 2), 12, 6),
            base.clone(),
        ];
        let (path, encoded) = record("timing", 30, &frames);
        let (declared, shown) = decode(&path);

        assert_eq!(encoded.frames, 6);
        assert_eq!(
            encoded.written, 4,
            "two repeats lengthen the frame before them"
        );
        assert_eq!(
            declared, 4,
            "`acTL` holds the count, patched once the stream ended"
        );
        assert_eq!(shown.len(), 4);
        let delays: Vec<_> = shown.iter().map(|frame| frame.delay).collect();
        assert_eq!(delays, [(1, 30), (3, 30), (1, 30), (1, 30)]);

        let mut at = 0;
        for frame in &shown {
            for _ in 0..frame.delay.0 {
                assert_eq!(frame.image, frames[at], "frame {at} as a viewer shows it");
                at += 1;
            }
        }
        assert_eq!(at, frames.len(), "the delays add up to the whole recording");
    }

    #[test]
    fn a_frame_writes_only_the_rectangle_that_changed() {
        let base = solid(32, 32, 0);
        let frames = [base.clone(), dotted(&dotted(&base, 4, 5), 9, 7)];
        let (path, _) = record("rectangle", 10, &frames);
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(&path).expect("file")));
        let mut reader = decoder.read_info().expect("header");
        let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
        reader.next_frame(&mut buffer).expect("the first frame");
        reader.next_frame(&mut buffer).expect("the second frame");
        let control = reader.info().frame_control().expect("a frame control");
        assert_eq!(
            (
                control.x_offset,
                control.y_offset,
                control.width,
                control.height
            ),
            (4, 5, 6, 3),
            "from (4, 5) to (9, 7)"
        );
    }

    #[test]
    fn a_still_recording_is_one_frame() {
        let frames = vec![solid(8, 8, 90); 12];
        let (path, encoded) = record("still", 24, &frames);
        let (declared, shown) = decode(&path);
        assert_eq!((encoded.written, declared, shown.len()), (1, 1, 1));
        assert_eq!(shown[0].delay, (12, 24));
    }

    #[test]
    fn a_run_longer_than_one_delay_holds_goes_on_without_changing_the_picture() {
        let frames = vec![solid(4, 4, 7); usize::from(u16::MAX) + 5];
        let (path, encoded) = record("long", 60, &frames);
        let (_, shown) = decode(&path);
        assert_eq!(encoded.written, 2);
        assert_eq!(
            shown.iter().map(|frame| frame.delay).collect::<Vec<_>>(),
            [(u16::MAX, 60), (5, 60)]
        );
        assert!(shown.iter().all(|frame| frame.image == frames[0]));
    }

    #[test]
    fn the_same_frames_write_the_same_bytes() {
        let base = solid(20, 10, 3);
        let frames = [base.clone(), dotted(&base, 1, 1), dotted(&base, 18, 9)];
        let (first, _) = record("same-a", 25, &frames);
        let (second, _) = record("same-b", 25, &frames);
        assert_eq!(
            std::fs::read(first).expect("first"),
            std::fs::read(second).expect("second")
        );
    }

    #[test]
    fn a_hold_lengthens_the_frame_before_it() {
        let base = solid(8, 8, 20);
        let path = scratch("hold");
        let mut sink = Recorder::create(&path, 10, VideoFormat::Apng).expect("a recording");
        sink.frame(base.clone()).expect("the first frame");
        sink.hold(2).expect("a hold");
        sink.frame(dotted(&base, 1, 1)).expect("the second frame");
        sink.hold(3).expect("a hold at the end");
        let encoded = sink.finish().expect("the recording closes");
        assert_eq!((encoded.frames, encoded.held), (2, 5));
        let (_, shown) = decode(&path);
        let delays: Vec<_> = shown.iter().map(|frame| frame.delay).collect();
        assert_eq!(delays, [(3, 10), (4, 10)]);
    }

    #[test]
    fn an_mp4_hold_repeats_the_last_frame_on_the_timeline() {
        let path = scratch("mp4-hold").with_extension("mp4");
        let mut sink = Recorder::create(&path, 10, VideoFormat::Mp4).expect("a recording");
        sink.frame(solid(16, 16, 9)).expect("a frame");
        sink.hold(4).expect("a hold");
        let encoded = sink.finish().expect("the recording closes");
        assert_eq!((encoded.frames, encoded.held, encoded.written), (1, 4, 5));
        let bytes = std::fs::read(&path).expect("the recording");
        let size = bytes.len() as u64;
        let mp4 = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
        let track = mp4.tracks().values().next().expect("a video track");
        assert_eq!(
            track.duration(),
            std::time::Duration::from_millis(500),
            "one frame and four held, at 10 fps"
        );
    }

    #[test]
    fn an_apng_grows_with_its_frames() {
        let small = solid(4, 4, 10);
        let wide = solid(8, 4, 20);
        let tall = solid(8, 8, 30);
        let frames = [
            small.clone(),
            small.clone(),
            dotted(&small, 3, 3),
            wide.clone(),
            tall.clone(),
            dotted(&tall, 7, 7),
        ];
        let path = scratch("grows");
        let mut sink = Recorder::growing(&path, 10).expect("a recording");
        for frame in frames {
            sink.frame(frame).expect("the encoder takes the frame");
        }
        let encoded = sink.finish().expect("the recording closes");
        let (declared, shown) = decode(&path);
        assert_eq!((encoded.written, declared, shown.len()), (5, 5, 5));
        let expected = [
            grown(&small, (8, 8)),
            grown(&dotted(&small, 3, 3), (8, 8)),
            grown(&wide, (8, 8)),
            tall.clone(),
            dotted(&tall, 7, 7),
        ];
        for (at, (shown, expected)) in shown.iter().zip(&expected).enumerate() {
            assert_eq!(shown.image, *expected, "frame {at}");
        }
        let delays: Vec<_> = shown.iter().map(|frame| frame.delay).collect();
        assert_eq!(delays, [(2, 10), (1, 10), (1, 10), (1, 10), (1, 10)]);
    }

    #[test]
    fn an_apng_becomes_an_mp4_of_its_last_size_with_every_frame_and_hold() {
        let small = solid(64, 32, 10);
        let tall = solid(64, 47, 30);
        let path = scratch("to-mp4");
        let mut sink = Recorder::growing(&path, 10).expect("a recording");
        for frame in [&small, &small, &tall] {
            sink.frame(frame.clone()).expect("a frame");
        }
        sink.hold(4).expect("a hold");
        sink.finish().expect("the recording closes");
        let mp4 = path.with_extension("mp4");
        let over = ["key", "key", "click", "click", "click", "gone"];
        let mut laid = Vec::new();
        let file = File::create(&mp4).expect("created");
        to_mp4(&path, file, &over, |shown, image| {
            assert_eq!(
                image.dimensions(),
                (64, 47),
                "every frame is of the last size"
            );
            laid.push(*shown);
            Ok(())
        })
        .expect("an MP4");
        assert_eq!(mp4_frames(&mp4), ((64, 48), 7));
        assert_eq!(laid, ["key", "click", "gone"], "once for each it is on");
    }

    #[test]
    fn a_frame_of_another_size_fails_a_recording_that_does_not_grow() {
        for format in VideoFormat::ALL {
            let mut sink = Recorder::create(&scratch("sizes"), 10, format).expect("a recording");
            sink.frame(solid(16, 16, 0))
                .expect("the first frame sets the size");
            sink.frame(solid(32, 16, 0))
                .expect("queued before the encoder sees it");
            let error = sink.finish().expect_err("the encoder refuses it");
            assert!(
                error.contains("the canvas went from 16×16 to 32×16 while it was recorded"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_recording_that_failed_says_why_however_often_it_is_asked() {
        let mut sink = Recorder::create(&scratch("failed"), 10, VideoFormat::Apng).expect("begun");
        sink.frame(solid(16, 16, 0)).expect("the first frame");
        let mut sizes = (17..).map(|width| sink.frame(solid(width, 16, 0)));
        let error = sizes.find_map(Result::err).expect("it stops");
        assert!(error.contains("the canvas went from"), "{error}");
        assert_eq!(sink.hold(1), Err(error.clone()));
        assert_eq!(sink.finish(), Err(error));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_recording_a_full_disk_has_no_room_for_is_an_error_however_short() {
        for format in VideoFormat::ALL {
            let mut sink =
                Recorder::create(Utf8Path::new("/dev/full"), 10, format).expect("a recording");
            sink.frame(solid(16, 16, 0)).expect("the first frame");
            let error = sink.finish().expect_err("nothing can be written");
            assert!(error.contains("No space left"), "{format:?}: {error}");
        }
        let apng = scratch("full");
        let mut sink = Recorder::growing(&apng, 10).expect("a recording");
        sink.frame(solid(16, 16, 0)).expect("the first frame");
        sink.finish().expect("the recording closes");
        let full = File::options().write(true).open("/dev/full");
        let kept = to_mp4::<()>(&apng, full.expect("opened"), &[], |(), _| Ok(()));
        let error = kept.expect_err("nothing can be written");
        assert!(error.contains("No space left"), "{error}");
    }

    #[test]
    fn a_growing_recording_stands_a_smaller_frame_in_its_corner() {
        let path = scratch("shrinks");
        let mut sink = Recorder::growing(&path, 10).expect("a recording");
        let (first, small, odd) = (solid(8, 8, 10), solid(4, 4, 90), solid(12, 2, 50));
        for frame in [&first, &small, &odd] {
            sink.frame(frame.clone()).expect("the encoder takes it");
        }
        sink.finish().expect("the recording closes");
        let (_, shown) = decode(&path);
        let shown: Vec<_> = shown.into_iter().map(|frame| frame.image).collect();
        assert_eq!(
            shown,
            [first, small, odd].map(|frame| grown(&frame, (12, 8)))
        );
    }

    #[test]
    fn a_recording_dropped_keeps_what_it_holds_unless_it_was_growing() {
        let frames = |mut sink: Recorder| {
            sink.frame(solid(4, 4, 0)).expect("the first frame");
            sink.frame(solid(4, 4, 9)).expect("the next frame");
        };
        let path = scratch("dropped");
        frames(Recorder::create(&path, 10, VideoFormat::Apng).expect("a recording"));
        assert_eq!(decode(&path).1.len(), 2);

        frames(Recorder::growing(&path, 10).expect("a recording"));
        let bytes = std::fs::read(&path).expect("the file");
        let width = bytes.get(16..20);
        assert_ne!(
            width,
            Some(&4_u32.to_be_bytes()[..]),
            "its header is not patched"
        );
    }

    #[test]
    fn an_mp4_holds_every_frame_at_exactly_its_time_with_a_keyframe_each_second() {
        let base = solid(64, 47, 30);
        let frames: Vec<_> = (0..61).map(|at| dotted(&base, at, at % 47)).collect();
        let path = scratch("mp4").with_extension("mp4");
        let mut sink = Recorder::create(&path, 30, VideoFormat::Mp4).expect("a recording");
        for frame in &frames {
            sink.frame(frame.clone())
                .expect("the encoder takes the frame");
        }
        let encoded = sink.finish().expect("the recording closes");
        assert_eq!(
            (encoded.frames, encoded.written),
            (61, 61),
            "every frame is encoded"
        );

        let bytes = std::fs::read(&path).expect("the recording");
        let size = bytes.len() as u64;
        let mut mp4 =
            mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
        let (&id, track) = mp4.tracks().iter().next().expect("a video track");
        assert_eq!(
            (track.width(), track.height()),
            (64, 48),
            "an odd height is padded to even"
        );
        let ticks_per_frame = track.timescale() / 30;
        assert_eq!(mp4.sample_count(id).expect("samples"), 61);
        let mut keyframes = Vec::new();
        for n in 1..=61 {
            let sample = mp4.read_sample(id, n).expect("a sample").expect("present");
            assert_eq!(
                sample.duration, ticks_per_frame,
                "sample {n} lasts one frame"
            );
            assert_eq!(
                sample.start_time,
                u64::from((n - 1) * ticks_per_frame),
                "sample {n} starts at its frame's time"
            );
            if sample.is_sync {
                keyframes.push(n - 1);
            }
        }
        assert_eq!(
            keyframes,
            [0, 30, 60],
            "a keyframe every second, for seeking"
        );
    }

    #[test]
    fn an_mp4_and_an_apng_of_the_same_frames_hold_the_same_number_of_seconds() {
        let frames = vec![solid(16, 16, 9); 20];
        let path = scratch("mp4-still").with_extension("mp4");
        let mut sink = Recorder::create(&path, 10, VideoFormat::Mp4).expect("a recording");
        for frame in &frames {
            sink.frame(frame.clone())
                .expect("the encoder takes the frame");
        }
        sink.finish().expect("the recording closes");
        let bytes = std::fs::read(&path).expect("the recording");
        let size = bytes.len() as u64;
        let mp4 = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
        let track = mp4.tracks().values().next().expect("a video track");
        assert_eq!(
            track.duration(),
            std::time::Duration::from_secs(2),
            "20 frames at 10 fps"
        );
    }

    /// The stage's checkerboard is a step of a few shades, the first detail a coarse quantiser smears away.
    #[test]
    fn an_mp4_keeps_a_low_contrast_pattern_sharp() {
        let checker = RgbaImage::from_fn(64, 64, |x, y| {
            let shade = if (x / 8 + y / 8) % 2 == 0 { 40 } else { 56 };
            image::Rgba([shade, shade, shade, 255])
        });
        let path = scratch("mp4-sharp").with_extension("mp4");
        let mut sink = Recorder::create(&path, 30, VideoFormat::Mp4).expect("a recording");
        sink.frame(checker.clone())
            .expect("the encoder takes the frame");
        sink.finish().expect("the recording closes");

        let frames = decode_mp4(&path);
        let rgb = frames[0].as_raw();
        assert_eq!(frames[0].dimensions(), (64, 64));
        let squared: f64 = checker
            .pixels()
            .flat_map(|pixel| &pixel.0[..3])
            .zip(rgb)
            .map(|(&want, &got)| (f64::from(want) - f64::from(got)).powi(2))
            .sum();
        let psnr = 10.0 * (255.0_f64.powi(2) / (squared / rgb.len() as f64)).log10();
        assert!(
            psnr >= 40.0,
            "the checkerboard comes back at {psnr:.1} dB, under 40"
        );
    }

    #[test]
    fn a_recording_of_nothing_is_an_error() {
        let sink = Recorder::create(&scratch("empty"), 10, VideoFormat::Apng).expect("a recording");
        let error = sink.finish().expect_err("no frames");
        assert!(error.contains("at least one frame"), "{error}");
    }
}
