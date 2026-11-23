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
    #[default]
    Apng,
    /// H.264: small, lossy, and seekable in any player.
    Mp4,
}

impl VideoFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Apng => "apng",
            Self::Mp4 => "mp4",
        }
    }

    /// The file's writer, started with the first frame.
    fn start(self, file: File, first: RgbaImage, fps: u16) -> Result<Box<dyn Stream>, String> {
        match self {
            Self::Apng => Ok(Box::new(Apng::start(file, first, fps)?)),
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

/// What the encoder thread is handed.
enum Message {
    Frame(RgbaImage),
    /// Show the last frame for this many frames more.
    Hold(u32),
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
}

impl Recorder {
    /// Start a recording into `path` at `fps`;
    /// the file is created now, so a bad path fails first.
    ///
    /// # Errors
    /// When `path` cannot be created or `fps` is zero.
    pub(crate) fn create(path: &Utf8Path, fps: u16, format: VideoFormat) -> Result<Self, String> {
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
                    encode(file, fps, format, &incoming, &queued)
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
        })
    }

    /// The encoder's own error, once it has stopped taking frames.
    fn failure(&mut self) -> String {
        self.frames = None;
        match self.worker.take().map(thread::JoinHandle::join) {
            Some(Ok(Err(error))) => error,
            Some(Err(_)) => "the encoder panicked".to_owned(),
            Some(Ok(Ok(_))) | None => "the encoder stopped early".to_owned(),
        }
    }

    /// Hand over the next frame, the same size as the first.
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
            return Err("the recording is already closed".to_owned());
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
        // Dropping the sender ends the encoder's loop.
        self.frames = None;
        let encoded = match self.worker.take().map(thread::JoinHandle::join) {
            Some(Ok(result)) => result?,
            Some(Err(_)) => return Err("the encoder panicked".to_owned()),
            None => return Err("the recording is already closed".to_owned()),
        };
        Ok(Encoded {
            frames: self.count,
            held: self.held,
            deepest_queue: self.deepest_queue,
            ..encoded
        })
    }
}

/// The encoder thread: every frame in, one file out.
fn encode(
    file: File,
    fps: u16,
    format: VideoFormat,
    incoming: &mpsc::Receiver<Message>,
    queued: &AtomicUsize,
) -> Result<Encoded, String> {
    let first = match incoming.recv() {
        Ok(Message::Frame(first)) => first,
        Ok(Message::Hold(_)) => return Err("a hold before any frame to hold".to_owned()),
        Err(_) => return Err("a recording needs at least one frame".to_owned()),
    };
    queued.fetch_sub(1, Ordering::Relaxed);
    let mut started = Instant::now();
    let mut stream = format.start(file, first, fps)?;
    let mut spent = started.elapsed();
    for message in incoming {
        queued.fetch_sub(1, Ordering::Relaxed);
        started = Instant::now();
        match message {
            Message::Frame(image) => stream.push(image)?,
            Message::Hold(frames) => stream.hold(frames)?,
        }
        spent += started.elapsed();
    }
    started = Instant::now();
    let written = stream.finish()?;
    spent += started.elapsed();
    Ok(Encoded {
        frames: 0,
        held: 0,
        written,
        encode: spent,
        deepest_queue: 0,
    })
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
}

impl Apng {
    fn start(file: File, first: RgbaImage, fps: u16) -> Result<Self, String> {
        let (width, height) = first.dimensions();
        let patch = file.try_clone().map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        // The count is unknown until the stream ends, and `acTL`
        // comes before any frame: a placeholder now, patched
        // by `set_frame_count` once the file is closed.
        encoder
            .set_animated(u32::MAX, 0)
            .map_err(|e| e.to_string())?;
        let writer = encoder.write_header().map_err(|e| e.to_string())?;
        Ok(Self {
            writer,
            file: patch,
            fps,
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
        if image.dimensions() != self.shown.dimensions() {
            return Err(format!(
                "a frame of {:?} in a recording of {:?}",
                image.dimensions(),
                self.shown.dimensions()
            ));
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
        set_frame_count(&mut apng.file, apng.written)?;
        Ok(apng.written)
    }
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
            return Err(format!(
                "a frame of {:?} in a recording of {:?}",
                image.dimensions(),
                self.size
            ));
        }
        self.last = Self::yuv(image, self.coded);
        self.write_last()
    }

    fn hold(&mut self, frames: u32) -> Result<(), String> {
        (0..frames).try_for_each(|_| self.write_last())
    }

    fn finish(self: Box<Self>) -> Result<u32, String> {
        let mp4 = *self;
        mp4.muxer.finish().map_err(|e| e.to_string())?;
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

/// Write the real frame count into the `acTL` chunk the encoder
/// left a placeholder in, with that chunk's CRC to match.
fn set_frame_count(file: &mut File, frames: u32) -> Result<(), String> {
    const SIGNATURE: u64 = 8;
    let io = |e: std::io::Error| e.to_string();
    let mut at = SIGNATURE;
    loop {
        file.seek(SeekFrom::Start(at)).map_err(io)?;
        let mut head = [0; 8];
        file.read_exact(&mut head).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => {
                "the file ended before its `acTL` chunk".to_owned()
            }
            _ => e.to_string(),
        })?;
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        if &head[4..] == b"acTL" {
            let mut data = [0; 8];
            file.read_exact(&mut data).map_err(io)?;
            data[..4].copy_from_slice(&frames.to_be_bytes());
            let mut crc = crc32fast::Hasher::new();
            crc.update(b"acTL");
            crc.update(&data);
            file.seek(SeekFrom::Start(at + 8)).map_err(io)?;
            file.write_all(&data).map_err(io)?;
            file.write_all(&crc.finalize().to_be_bytes()).map_err(io)?;
            return file.flush().map_err(io);
        }
        if &head[4..] == b"IDAT" {
            return Err("no `acTL` chunk before the first frame".to_owned());
        }
        // Length, type, data and CRC.
        at += 12 + u64::from(length);
    }
}

/// A written frame as a viewer shows it:
/// composited over the ones before.
#[cfg(test)]
pub(crate) struct Shown {
    pub(crate) image: RgbaImage,
    pub(crate) delay: (u16, u16),
}

/// The frame count `acTL` declares,
/// and every frame as a viewer shows it.
#[cfg(test)]
pub(crate) fn decode(path: &Utf8Path) -> (u32, Vec<Shown>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(
        File::open(path).expect("the recording"),
    ));
    let mut reader = decoder.read_info().expect("a PNG header");
    let declared = reader
        .info()
        .animation_control
        .expect("an animated PNG")
        .num_frames;
    let (width, height) = (reader.info().width, reader.info().height);
    let mut canvas = RgbaImage::new(width, height);
    let mut buffer = vec![0; reader.output_buffer_size().expect("a buffer size")];
    let mut shown = Vec::new();
    while let Ok(output) = reader.next_frame(&mut buffer) {
        let control = *reader.info().frame_control().expect("a frame control");
        let frame = RgbaImage::from_raw(
            output.width,
            output.height,
            buffer[..output.buffer_size()].to_vec(),
        )
        .expect("frame pixels");
        image::imageops::replace(
            &mut canvas,
            &frame,
            i64::from(control.x_offset),
            i64::from(control.y_offset),
        );
        shown.push(Shown {
            image: canvas.clone(),
            delay: (control.delay_num, control.delay_den),
        });
    }
    (declared, shown)
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
    fn a_frame_of_another_size_fails_the_recording() {
        let path = scratch("sizes");
        let mut sink = Recorder::create(&path, 10, VideoFormat::Apng).expect("a recording");
        sink.frame(solid(4, 4, 0))
            .expect("the first frame sets the size");
        sink.frame(solid(5, 4, 0))
            .expect("queued before the encoder sees it");
        let error = sink.finish().expect_err("the encoder refuses it");
        assert!(
            error.contains("a frame of (5, 4) in a recording of (4, 4)"),
            "{error}"
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
        use openh264::formats::YUVSource;

        let checker = RgbaImage::from_fn(64, 64, |x, y| {
            let shade = if (x / 8 + y / 8) % 2 == 0 { 40 } else { 56 };
            image::Rgba([shade, shade, shade, 255])
        });
        let path = scratch("mp4-sharp").with_extension("mp4");
        let mut sink = Recorder::create(&path, 30, VideoFormat::Mp4).expect("a recording");
        sink.frame(checker.clone())
            .expect("the encoder takes the frame");
        sink.finish().expect("the recording closes");

        let bytes = std::fs::read(&path).expect("the recording");
        let size = bytes.len() as u64;
        let mut mp4 =
            mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).expect("an MP4");
        let (&id, track) = mp4.tracks().iter().next().expect("a video track");
        let start = [0, 0, 0, 1];
        let mut annex_b = [
            &start,
            track.sequence_parameter_set().expect("an SPS"),
            &start,
            track.picture_parameter_set().expect("a PPS"),
        ]
        .concat();
        let sample = mp4.read_sample(id, 1).expect("a sample").expect("present");
        let mut units = &sample.bytes[..];
        while let Some((length, rest)) = units.split_first_chunk::<4>() {
            let (unit, rest) = rest.split_at(u32::from_be_bytes(*length) as usize);
            annex_b.extend_from_slice(&start);
            annex_b.extend_from_slice(unit);
            units = rest;
        }

        let mut decoder = openh264::decoder::Decoder::new().expect("a decoder");
        let decoded = decoder
            .decode(&annex_b)
            .expect("the keyframe decodes")
            .expect("a picture");
        assert_eq!(decoded.dimensions(), (64, 64));
        let mut rgb = vec![0; 64 * 64 * 3];
        decoded.write_rgb8(&mut rgb);

        let squared: f64 = checker
            .pixels()
            .flat_map(|pixel| &pixel.0[..3])
            .zip(&rgb)
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
