//! Owning wrappers over the NDK media API (`libmediandk`): each holds one raw object and deletes
//! it on drop.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use ndk_sys as sys;

/// A failed media call or an unusable stream.
#[derive(Clone, Debug)]
pub struct MediaError(pub String);

impl MediaError {
    pub fn new(message: impl Into<String>) -> MediaError {
        MediaError(message.into())
    }
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MediaError {}

impl From<String> for MediaError {
    fn from(message: String) -> MediaError {
        MediaError(message)
    }
}

/// The frame picker's error type.
impl From<MediaError> for String {
    fn from(e: MediaError) -> String {
        e.0
    }
}

/// `Err` for a negative status of `call`.
fn check(call: &str, status: sys::media_status_t) -> Result<(), MediaError> {
    if status.0 < 0 {
        Err(MediaError(format!("{call}: status {}", status.0)))
    } else {
        Ok(())
    }
}

/// An in-memory movie as a media data source.
pub struct Source {
    raw: NonNull<sys::AMediaDataSource>,
    /// The callbacks' userdata: boxed, so its address is stable.
    bytes: Box<Arc<[u8]>>,
}

impl Source {
    pub fn new(bytes: Arc<[u8]>) -> Result<Source, MediaError> {
        // SAFETY: a plain constructor; null is handled.
        let raw = NonNull::new(unsafe { sys::AMediaDataSource_new() })
            .ok_or_else(|| MediaError::new("AMediaDataSource_new failed"))?;
        let source = Source {
            raw,
            bytes: Box::new(bytes),
        };
        let userdata = ptr::from_ref::<Arc<[u8]>>(&source.bytes)
            .cast_mut()
            .cast::<c_void>();
        // SAFETY: `raw` is valid; `userdata` lives as long as `source`, which deletes `raw`
        // before freeing it, and the callbacks only read through it.
        unsafe {
            sys::AMediaDataSource_setUserdata(raw.as_ptr(), userdata);
            sys::AMediaDataSource_setReadAt(raw.as_ptr(), Some(read_at));
            sys::AMediaDataSource_setGetSize(raw.as_ptr(), Some(get_size));
            sys::AMediaDataSource_setClose(raw.as_ptr(), Some(close));
        }
        Ok(source)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `AMediaDataSource_new` and is deleted once.
        unsafe { sys::AMediaDataSource_delete(self.raw.as_ptr()) };
    }
}

/// Copies up to `size` bytes at `offset`: the count, 0 at or past the end, -1 for a negative
/// offset. The framework's data source fills its buffer in a loop that ends on 0 and fails on a
/// negative count (despite the NDK header's "-1 is end of stream"), and MP4 indexes often sit at
/// the very end of the file.
unsafe extern "C" fn read_at(
    userdata: *mut c_void,
    offset: sys::off64_t,
    buffer: *mut c_void,
    size: usize,
) -> isize {
    // SAFETY: `userdata` is the `Arc<[u8]>` of the live `Source` (see `Source::new`).
    let bytes: &[u8] = unsafe { &*userdata.cast::<Arc<[u8]>>() };
    let Ok(offset) = usize::try_from(offset) else {
        return -1;
    };
    let rest = bytes.get(offset..).unwrap_or_default();
    let n = size.min(rest.len());
    // SAFETY: the extractor gives a buffer of at least `size` bytes; `rest` has `n`.
    unsafe { ptr::copy_nonoverlapping(rest.as_ptr(), buffer.cast::<u8>(), n) };
    isize::try_from(n).unwrap_or(-1)
}

unsafe extern "C" fn get_size(userdata: *mut c_void) -> isize {
    // SAFETY: as in `read_at`.
    let bytes: &[u8] = unsafe { &*userdata.cast::<Arc<[u8]>>() };
    isize::try_from(bytes.len()).unwrap_or(-1)
}

unsafe extern "C" fn close(_userdata: *mut c_void) {}

/// A media format (key-value pairs).
pub struct Format(NonNull<sys::AMediaFormat>);

impl Format {
    fn from_raw(raw: *mut sys::AMediaFormat) -> Option<Format> {
        NonNull::new(raw).map(Format)
    }

    pub fn i32(&self, key: &CStr) -> Option<i32> {
        let mut out = 0;
        // SAFETY: `self.0` is a live format, `key` a C string, `out` a valid pointer.
        unsafe { sys::AMediaFormat_getInt32(self.0.as_ptr(), key.as_ptr(), &mut out) }
            .then_some(out)
    }

    pub fn i64(&self, key: &CStr) -> Option<i64> {
        let mut out = 0;
        // SAFETY: as in `i32`.
        unsafe { sys::AMediaFormat_getInt64(self.0.as_ptr(), key.as_ptr(), &mut out) }
            .then_some(out)
    }

    pub fn string(&self, key: &CStr) -> Option<String> {
        let mut out: *const c_char = ptr::null();
        // SAFETY: as in `i32`; the string belongs to the format and is copied at once.
        unsafe {
            if !sys::AMediaFormat_getString(self.0.as_ptr(), key.as_ptr(), &mut out) || out.is_null()
            {
                return None;
            }
            Some(CStr::from_ptr(out).to_string_lossy().into_owned())
        }
    }

    pub fn set_i32(&mut self, key: &CStr, value: i32) {
        // SAFETY: as in `i32`.
        unsafe { sys::AMediaFormat_setInt32(self.0.as_ptr(), key.as_ptr(), value) };
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: the format is owned and deleted once.
        unsafe { sys::AMediaFormat_delete(self.0.as_ptr()) };
    }
}

/// A demuxer over an in-memory movie.
pub struct Extractor {
    raw: NonNull<sys::AMediaExtractor>,
    /// Dropped after `raw` is deleted (`Drop` runs before the fields drop).
    _source: Source,
}

impl Extractor {
    pub fn new(bytes: Arc<[u8]>) -> Result<Extractor, MediaError> {
        let source = Source::new(bytes)?;
        // SAFETY: a plain constructor; null is handled.
        let raw = NonNull::new(unsafe { sys::AMediaExtractor_new() })
            .ok_or_else(|| MediaError::new("AMediaExtractor_new failed"))?;
        let extractor = Extractor {
            raw,
            _source: source,
        };
        // SAFETY: both objects are live; the extractor is deleted before the source.
        check("AMediaExtractor_setDataSourceCustom", unsafe {
            sys::AMediaExtractor_setDataSourceCustom(raw.as_ptr(), extractor._source.raw.as_ptr())
        })?;
        Ok(extractor)
    }

    pub fn track_count(&self) -> usize {
        // SAFETY: `raw` is live.
        unsafe { sys::AMediaExtractor_getTrackCount(self.raw.as_ptr()) }
    }

    pub fn track_format(&self, index: usize) -> Option<Format> {
        // SAFETY: `raw` is live; the returned format is owned by the caller.
        Format::from_raw(unsafe { sys::AMediaExtractor_getTrackFormat(self.raw.as_ptr(), index) })
    }

    /// The container's format (its `durationUs`).
    pub fn file_format(&self) -> Option<Format> {
        // SAFETY: as in `track_format`.
        Format::from_raw(unsafe { sys::AMediaExtractor_getFileFormat(self.raw.as_ptr()) })
    }

    pub fn select(&mut self, index: usize) -> Result<(), MediaError> {
        // SAFETY: `raw` is live.
        check("AMediaExtractor_selectTrack", unsafe {
            sys::AMediaExtractor_selectTrack(self.raw.as_ptr(), index)
        })
    }

    /// Copies the current sample into `buffer`; its size, or negative at the end.
    pub fn read_sample(&mut self, buffer: &mut [u8]) -> isize {
        // SAFETY: `raw` is live; `buffer` is writable for its length.
        unsafe {
            sys::AMediaExtractor_readSampleData(
                self.raw.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        }
    }

    /// The current sample's presentation time (µs); negative at the end.
    pub fn sample_time_us(&self) -> i64 {
        // SAFETY: `raw` is live.
        unsafe { sys::AMediaExtractor_getSampleTime(self.raw.as_ptr()) }
    }

    /// Moves to the next sample; false at the end.
    pub fn advance(&mut self) -> bool {
        // SAFETY: `raw` is live.
        unsafe { sys::AMediaExtractor_advance(self.raw.as_ptr()) }
    }

    /// Moves to the sync sample at or before `us`.
    pub fn seek_us(&mut self, us: i64) -> Result<(), MediaError> {
        // SAFETY: `raw` is live.
        check("AMediaExtractor_seekTo", unsafe {
            sys::AMediaExtractor_seekTo(
                self.raw.as_ptr(),
                us,
                sys::SeekMode::AMEDIAEXTRACTOR_SEEK_PREVIOUS_SYNC,
            )
        })
    }
}

impl Drop for Extractor {
    fn drop(&mut self) {
        // SAFETY: the extractor is owned and deleted once, before its source.
        unsafe { sys::AMediaExtractor_delete(self.raw.as_ptr()) };
    }
}

/// What `Codec::dequeue_output` gave.
pub enum Output {
    Buffer {
        index: usize,
        pts_us: i64,
        size: usize,
        offset: usize,
        eos: bool,
    },
    FormatChanged,
    TryAgain,
}

/// A decoder in synchronous mode.
pub struct Codec(NonNull<sys::AMediaCodec>);

impl Codec {
    /// The platform's preferred decoder for `mime` (hardware when there is one).
    pub fn by_type(mime: &str) -> Result<Codec, MediaError> {
        let c = CString::new(mime).map_err(|e| MediaError(e.to_string()))?;
        // SAFETY: `c` is a C string; null is handled.
        NonNull::new(unsafe { sys::AMediaCodec_createDecoderByType(c.as_ptr()) })
            .map(Codec)
            .ok_or_else(|| MediaError(format!("no decoder for {mime}")))
    }

    /// The codec called `name`.
    pub fn by_name(name: &str) -> Result<Codec, MediaError> {
        let c = CString::new(name).map_err(|e| MediaError(e.to_string()))?;
        // SAFETY: as in `by_type`.
        NonNull::new(unsafe { sys::AMediaCodec_createCodecByName(c.as_ptr()) })
            .map(Codec)
            .ok_or_else(|| MediaError(format!("no codec {name}")))
    }

    /// Configures decoding of `format`, into `surface` when given (else into byte buffers).
    pub fn configure(
        &mut self,
        format: &Format,
        surface: Option<NonNull<sys::ANativeWindow>>,
    ) -> Result<(), MediaError> {
        let window = surface.map_or(ptr::null_mut(), NonNull::as_ptr);
        // SAFETY: the codec and format are live; the window outlives the configuration (the
        // owner drops the codec before the reader).
        check("AMediaCodec_configure", unsafe {
            sys::AMediaCodec_configure(
                self.0.as_ptr(),
                format.0.as_ptr(),
                window,
                ptr::null_mut(),
                0,
            )
        })
    }

    pub fn start(&mut self) -> Result<(), MediaError> {
        // SAFETY: the codec is live.
        check("AMediaCodec_start", unsafe {
            sys::AMediaCodec_start(self.0.as_ptr())
        })
    }

    /// Drops every queued input and output buffer.
    pub fn flush(&mut self) -> Result<(), MediaError> {
        // SAFETY: the codec is live.
        check("AMediaCodec_flush", unsafe {
            sys::AMediaCodec_flush(self.0.as_ptr())
        })
    }

    /// The codec's name, such as `c2.android.avc.decoder`.
    pub fn name(&self) -> String {
        let mut out: *mut c_char = ptr::null_mut();
        // SAFETY: the codec is live; the name is copied, then released.
        unsafe {
            if sys::AMediaCodec_getName(self.0.as_ptr(), &mut out).0 < 0 || out.is_null() {
                return "unknown".to_owned();
            }
            let name = CStr::from_ptr(out).to_string_lossy().into_owned();
            sys::AMediaCodec_releaseName(self.0.as_ptr(), out);
            name
        }
    }

    /// Moves the output to another surface.
    pub fn set_output_surface(
        &mut self,
        surface: NonNull<sys::ANativeWindow>,
    ) -> Result<(), MediaError> {
        // SAFETY: the codec and window are live.
        check("AMediaCodec_setOutputSurface", unsafe {
            sys::AMediaCodec_setOutputSurface(self.0.as_ptr(), surface.as_ptr())
        })
    }

    pub fn output_format(&self) -> Option<Format> {
        // SAFETY: the codec is live; the format is owned by the caller.
        Format::from_raw(unsafe { sys::AMediaCodec_getOutputFormat(self.0.as_ptr()) })
    }

    /// A free input buffer's index; `None` when none is free within `timeout_us`.
    pub fn dequeue_input(&mut self, timeout_us: i64) -> Result<Option<usize>, MediaError> {
        // SAFETY: the codec is live.
        let index = unsafe { sys::AMediaCodec_dequeueInputBuffer(self.0.as_ptr(), timeout_us) };
        match usize::try_from(index) {
            Ok(index) => Ok(Some(index)),
            Err(_) if index == -1 => Ok(None),
            Err(_) => Err(MediaError(format!(
                "AMediaCodec_dequeueInputBuffer: status {index}"
            ))),
        }
    }

    /// The memory of a dequeued input buffer.
    pub fn input_buffer(&mut self, index: usize) -> Result<&mut [u8], MediaError> {
        let mut size = 0;
        // SAFETY: the codec is live; the buffer stays valid until it is queued, which needs
        // `&mut self` again.
        unsafe {
            let data = sys::AMediaCodec_getInputBuffer(self.0.as_ptr(), index, &mut size);
            if data.is_null() {
                return Err(MediaError::new("AMediaCodec_getInputBuffer failed"));
            }
            Ok(std::slice::from_raw_parts_mut(data, size))
        }
    }

    pub fn queue_input(
        &mut self,
        index: usize,
        size: usize,
        pts_us: i64,
        flags: u32,
    ) -> Result<(), MediaError> {
        let time = u64::try_from(pts_us).unwrap_or(0);
        // SAFETY: the codec is live; `index` was dequeued.
        check("AMediaCodec_queueInputBuffer", unsafe {
            sys::AMediaCodec_queueInputBuffer(self.0.as_ptr(), index, 0, size, time, flags)
        })
    }

    /// The next output event within `timeout_us`.
    pub fn dequeue_output(&mut self, timeout_us: i64) -> Result<Output, MediaError> {
        let mut info = sys::AMediaCodecBufferInfo {
            offset: 0,
            size: 0,
            presentationTimeUs: 0,
            flags: 0,
        };
        // SAFETY: the codec is live; `info` is a valid pointer.
        let index =
            unsafe { sys::AMediaCodec_dequeueOutputBuffer(self.0.as_ptr(), &mut info, timeout_us) };
        if let Ok(index) = usize::try_from(index) {
            return Ok(Output::Buffer {
                index,
                pts_us: info.presentationTimeUs,
                size: usize::try_from(info.size).unwrap_or(0),
                offset: usize::try_from(info.offset).unwrap_or(0),
                eos: info.flags & sys::AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM != 0,
            });
        }
        let status = c_int::try_from(index).unwrap_or(c_int::MIN);
        match status {
            sys::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED => Ok(Output::FormatChanged),
            sys::AMEDIACODEC_INFO_TRY_AGAIN_LATER | sys::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {
                Ok(Output::TryAgain)
            }
            _ => Err(MediaError(format!(
                "AMediaCodec_dequeueOutputBuffer: status {index}"
            ))),
        }
    }

    /// The memory of a dequeued output buffer (byte-buffer mode).
    pub fn output_buffer(&self, index: usize) -> Option<&[u8]> {
        let mut size = 0;
        // SAFETY: the codec is live; the buffer stays valid until released, which needs
        // `&mut self`.
        unsafe {
            let data = sys::AMediaCodec_getOutputBuffer(self.0.as_ptr(), index, &mut size);
            (!data.is_null()).then(|| std::slice::from_raw_parts(data.cast_const(), size))
        }
    }

    /// Gives an output buffer back, sending it to the surface when `render`.
    pub fn release_output(&mut self, index: usize, render: bool) -> Result<(), MediaError> {
        // SAFETY: the codec is live; `index` was dequeued.
        check("AMediaCodec_releaseOutputBuffer", unsafe {
            sys::AMediaCodec_releaseOutputBuffer(self.0.as_ptr(), index, render)
        })
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: the codec is owned, stopped and deleted once.
        unsafe {
            sys::AMediaCodec_stop(self.0.as_ptr());
            sys::AMediaCodec_delete(self.0.as_ptr());
        }
    }
}

/// A queue of decoded YUV 4:2:0 pictures a decoder renders into.
pub struct Reader {
    raw: NonNull<sys::AImageReader>,
    size: (i32, i32),
}

impl Reader {
    pub fn new(width: i32, height: i32) -> Result<Reader, MediaError> {
        let mut raw = ptr::null_mut();
        // SAFETY: `raw` is a valid out pointer.
        check("AImageReader_new", unsafe {
            sys::AImageReader_new(
                width,
                height,
                sys::AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32,
                2,
                &mut raw,
            )
        })?;
        NonNull::new(raw)
            .map(|raw| Reader {
                raw,
                size: (width, height),
            })
            .ok_or_else(|| MediaError::new("AImageReader_new gave no reader"))
    }

    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// The reader's input surface, valid while the reader lives.
    pub fn window(&self) -> Result<NonNull<sys::ANativeWindow>, MediaError> {
        let mut window = ptr::null_mut();
        // SAFETY: the reader is live; `window` is a valid out pointer.
        check("AImageReader_getWindow", unsafe {
            sys::AImageReader_getWindow(self.raw.as_ptr(), &mut window)
        })?;
        NonNull::new(window).ok_or_else(|| MediaError::new("AImageReader_getWindow gave none"))
    }

    /// The oldest picture not acquired yet; `None` when there is none.
    pub fn acquire_next(&self) -> Result<Option<Image<'_>>, MediaError> {
        let mut image = ptr::null_mut();
        // SAFETY: the reader is live; `image` is a valid out pointer.
        let status = unsafe { sys::AImageReader_acquireNextImage(self.raw.as_ptr(), &mut image) };
        if status == sys::media_status_t::AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE {
            return Ok(None);
        }
        check("AImageReader_acquireNextImage", status)?;
        Ok(NonNull::new(image).map(|raw| Image {
            raw,
            _reader: std::marker::PhantomData,
        }))
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: the reader is owned and deleted once; its images borrow it, so none is left.
        unsafe { sys::AImageReader_delete(self.raw.as_ptr()) };
    }
}

/// An acquired picture; deleted (given back to its reader) on drop.
pub struct Image<'a> {
    raw: NonNull<sys::AImage>,
    _reader: std::marker::PhantomData<&'a Reader>,
}

impl Image<'_> {
    /// Plane `index`'s bytes, row stride and pixel stride.
    pub fn plane(&self, index: c_int) -> Result<(&[u8], usize, usize), MediaError> {
        let (mut data, mut len, mut row, mut pixel) = (ptr::null_mut(), 0, 0, 0);
        // SAFETY: the image is live; the out pointers are valid; the plane memory lives as long
        // as the image, which the returned slice borrows.
        unsafe {
            check(
                "AImage_getPlaneData",
                sys::AImage_getPlaneData(self.raw.as_ptr(), index, &mut data, &mut len),
            )?;
            check(
                "AImage_getPlaneRowStride",
                sys::AImage_getPlaneRowStride(self.raw.as_ptr(), index, &mut row),
            )?;
            check(
                "AImage_getPlanePixelStride",
                sys::AImage_getPlanePixelStride(self.raw.as_ptr(), index, &mut pixel),
            )?;
            if data.is_null() {
                return Err(MediaError::new("AImage_getPlaneData gave no data"));
            }
            let bytes = std::slice::from_raw_parts(
                data.cast_const(),
                usize::try_from(len).unwrap_or(0),
            );
            Ok((
                bytes,
                usize::try_from(row).unwrap_or(0),
                usize::try_from(pixel).unwrap_or(0),
            ))
        }
    }

    /// The visible rectangle: left, top, right, bottom (exclusive).
    pub fn crop(&self) -> Result<(i32, i32, i32, i32), MediaError> {
        let mut rect = sys::AImageCropRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: the image is live; `rect` is a valid out pointer.
        check("AImage_getCropRect", unsafe {
            sys::AImage_getCropRect(self.raw.as_ptr(), &mut rect)
        })?;
        Ok((rect.left, rect.top, rect.right, rect.bottom))
    }
}

impl Drop for Image<'_> {
    fn drop(&mut self) {
        // SAFETY: the image is owned and deleted once.
        unsafe { sys::AImage_delete(self.raw.as_ptr()) };
    }
}
