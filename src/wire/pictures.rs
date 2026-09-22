//! The pictures inside an article: fetching them, naming them, decoding
//! them, and sweeping what is left on disk.
//!
//! The core's half of the reader's pictures. Nothing here draws anything --
//! that is `ui/pictures.rs` -- and nothing here knows which rectangle a
//! picture is going into: it is handed a box in pixels, hands back pixels
//! that fit it, and the window does the placing.
//!
//! Four decisions carry this file.
//!
//! **The bytes are hostile.** They arrived over a socket from the server
//! whose page was just scraped. [`starkit::graphics::decode_limited`] reads
//! the header and checks the declared size before a decoder is allowed to
//! believe it: a four-hundred-byte PNG that says it is forty thousand pixels
//! on a side would otherwise be six gigabytes of allocation on a thread the
//! reader is waiting for. There is a byte cap on the way in as well, because
//! a picture is not a download.
//!
//! **A file is named by a hash of its URL.** Not because the URL is secret,
//! but because it is an arbitrary string somebody else published: a filename
//! built out of one can hold a `/`, a `..`, a null byte or four kilobytes of
//! query string, and exactly one of those has to be wrong for the cache to
//! write outside its own directory. A hash cannot.
//!
//! **What is stored is what arrived.** The file on disk is the original
//! bytes, not the pixels that were drawn: the same picture is asked for
//! again at a different size every time the reader is widened, and a cache
//! of thumbnails would answer the second question with the first one's
//! answer. It is also what `[reading] click_picture` hands to an image
//! viewer, and a viewer opening a thumbnail of the file it was asked for is
//! worse than no cache at all.
//!
//! **Down, never up.** A picture smaller than the box it was given keeps its
//! own size. Blowing a 160-pixel logo up across a third of the reader is not
//! a service; the overlay is where a picture is made larger, and it does it
//! from these same pixels.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use url::Url;

use super::net::{Http, RequestOptions};

/// The most bytes one picture may be. Above this it is not an illustration.
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The largest picture worth decoding, on a side. STAR/KIT's own number.
pub const MAX_DIMENSION: u32 = starkit::graphics::MAX_DIMENSION;

/// How many pictures are fetched at once.
///
/// Two, and deliberately not more. The lane they run on is the one a refresh
/// of forty-one feeds is already using, and an article's pictures are worth
/// exactly as much of it as it takes to draw the screen: the reader asks for
/// what is on it and two screens ahead, so the third picture starting a
/// quarter of a second later is invisible, and four threads on one host is
/// not.
pub const PARALLEL: usize = 2;

/// How many decoded pictures are kept in memory.
///
/// An article with more than thirty-two pictures in it is a gallery, and the
/// ones scrolled past are the ones to let go of. The bytes are still on
/// disk, so coming back to one costs a decode rather than a request.
pub const CAPACITY: usize = 32;

/// The extensions the cache knows how to write, and therefore how to find
/// again.
///
/// A lookup has no `Content-Type` to go on, so it probes these in order
/// rather than listing the directory: five `stat` calls beats walking a
/// directory of a thousand files for every picture on screen.
const EXTENSIONS: &[&str] = &["png", "jpg", "webp", "gif", "bin"];

/// A decoded picture.
///
/// `Debug` is written by hand: the derived one on `RgbaImage` prints every
/// pixel, which turns one `{:?}` on a job into several megabytes of log.
#[derive(Clone)]
pub struct Picture {
    /// The pixels, scaled down to the box that was asked for.
    pub image: Arc<starkit::image::RgbaImage>,
    /// The size on the wire, before any scaling. What the overlay's footer
    /// says and what the layout reserves rows from.
    pub natural: (u32, u32),
    /// Where the original bytes are, when they were cached. What a click
    /// hands to an image viewer.
    pub path: Option<PathBuf>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Picture({}x{} of {}x{}{})",
            self.image.width(),
            self.image.height(),
            self.natural.0,
            self.natural.1,
            match &self.path {
                Some(p) => format!(", {}", p.display()),
                None => String::new(),
            }
        )
    }
}

/// The file name a URL is stored under: a SHA-256 of it, in hex.
///
/// The hash is of the URL exactly as the article carried it. Nothing is
/// stripped: a query string on a picture URL is usually a size or a format,
/// and two URLs that differ in one are two different pictures.
pub fn cache_name(url: &str, extension: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    format!("{digest:x}.{extension}")
}

/// The cached file for a URL, if there is one.
pub fn find_cached(dir: &Path, url: &str) -> Option<PathBuf> {
    for extension in EXTENSIONS {
        let path = dir.join(cache_name(url, extension));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// What to store a response under.
///
/// The `Content-Type` first, because it is what the server says it sent; the
/// bytes' own magic number second, because a CDN answering
/// `application/octet-stream` is still serving a PNG; `bin` last, so
/// something unrecognised is still cached rather than fetched for ever.
/// The URL's tail is deliberately not consulted: a path ending `.jpg` that
/// answers with a WebP is a real thing a CDN does, and the file would then
/// be found under a name that lies about it.
pub fn ext_for(content_type: Option<&str>, bytes: &[u8]) -> &'static str {
    let declared = content_type
        .map(|t| {
            t.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .and_then(|t| match t.as_str() {
            "image/png" | "image/apng" => Some("png"),
            "image/jpeg" | "image/jpg" => Some("jpg"),
            "image/webp" => Some("webp"),
            "image/gif" => Some("gif"),
            _ => None,
        });
    if let Some(extension) = declared {
        return extension;
    }
    match starkit::image::guess_format(bytes) {
        Ok(starkit::image::ImageFormat::Png) => "png",
        Ok(starkit::image::ImageFormat::Jpeg) => "jpg",
        Ok(starkit::image::ImageFormat::WebP) => "webp",
        Ok(starkit::image::ImageFormat::Gif) => "gif",
        _ => "bin",
    }
}

/// The size a `natural` picture is drawn at inside a box of pixels.
///
/// Down only, and by the smaller of the two ratios so the shape is kept. A
/// zero anywhere means "do not scale": a box nothing has measured is not a
/// box.
pub fn size_for(natural: (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = natural;
    if w == 0 || h == 0 || max_w == 0 || max_h == 0 {
        return (w, h);
    }
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let scale = (f64::from(max_w) / f64::from(w)).min(f64::from(max_h) / f64::from(h));
    (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    )
}

/// The filter a picture is scaled down with.
///
/// Triangle rather than Lanczos3 or Catmull-Rom, which is the opposite of
/// what the overlay uses and for the opposite reason: this runs on every
/// picture of every article as it is opened, downscaling is where a cheap
/// filter is hardest to tell from an expensive one, and the sharpening
/// filters ring on the hard edges a screenshot of text is made of.
const SHRINK: starkit::image::imageops::FilterType = starkit::image::imageops::FilterType::Triangle;

/// Turn bytes into pixels no larger than `max_w` by `max_h`.
pub fn decode(
    bytes: &[u8],
    max_w: u32,
    max_h: u32,
) -> Result<(Arc<starkit::image::RgbaImage>, (u32, u32)), String> {
    let decoded = starkit::graphics::decode_limited(bytes, MAX_DIMENSION)
        .map_err(|e| format!("it could not be decoded: {e}"))?;
    let image = decoded.into_rgba8();
    let natural = image.dimensions();
    if natural.0 == 0 || natural.1 == 0 {
        return Err("it has no pixels".into());
    }
    let (w, h) = size_for(natural, max_w, max_h);
    let image = if (w, h) == natural {
        image
    } else {
        starkit::image::imageops::resize(&image, w, h, SHRINK)
    };
    Ok((Arc::new(image), natural))
}

/// Fetch a picture -- or read the one already on disk -- and decode it to
/// fit `max_w` by `max_h`.
///
/// The error is a sentence rather than a type: everything this can fail with
/// ends up in the same place, which is the line drawn where the picture
/// would have been.
pub fn fetch(
    http: &dyn Http,
    dir: Option<&Path>,
    url: &str,
    max_w: u32,
    max_h: u32,
) -> Result<Picture, String> {
    let parsed = Url::parse(url).map_err(|e| format!("{url}: {e}"))?;

    if let Some(dir) = dir {
        if let Some(path) = find_cached(dir, url) {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let (image, natural) = decode(&bytes, max_w, max_h)?;
                    // Touched so the sweep sees it as recently wanted: a
                    // picture looked at every day should outlive one fetched
                    // once a month ago. Best effort -- a read-only cache is
                    // not a reason to fail a picture that has arrived.
                    let _ = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .and_then(|f| f.set_modified(SystemTime::now()));
                    return Ok(Picture {
                        image,
                        natural,
                        path: Some(path),
                    });
                }
                // A file that cannot be read is treated as one that is not
                // there. Falling through fetches it again and replaces it,
                // which is the right answer for a truncated cache entry.
                Err(e) => tracing::debug!("re-reading {}: {e}", path.display()),
            }
        }
    }

    let response = http
        .get(&parsed, &RequestOptions::picture(MAX_BYTES))
        .map_err(|e| e.to_string())?;
    if !response.is_ok() {
        return Err(format!(
            "{} answered {}",
            parsed.host_str().unwrap_or(url),
            response.status
        ));
    }

    let path = dir.and_then(|dir| {
        let extension = ext_for(response.content_type.as_deref(), &response.body);
        let path = dir.join(cache_name(url, extension));
        match starkit::fs::write_atomic(&path, &response.body) {
            Ok(()) => Some(path),
            // A cache that cannot be written is a reader that fetches more
            // than it should, not a reader that cannot show a picture.
            Err(e) => {
                tracing::warn!("caching {}: {e}", path.display());
                None
            }
        }
    });

    let (image, natural) = decode(&response.body, max_w, max_h)?;
    Ok(Picture {
        image,
        natural,
        path,
    })
}

/// Delete what the cache should no longer be holding: anything older than
/// `keep_days`, and then the oldest of what is left until the directory is
/// under `max_bytes`. Returns how many bytes went.
///
/// By modified time, which the filesystem already keeps, rather than by a
/// recorded access count in a sidecar index -- a second thing to get out of
/// step with the directory it describes.
pub fn sweep(dir: &Path, max_bytes: u64, keep_days: u32, now: SystemTime) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut entries: Vec<(PathBuf, u64, SystemTime)> = read
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((
                entry.path(),
                meta.len(),
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        })
        .collect();

    let mut total: u64 = entries.iter().map(|(_, size, _)| size).sum();
    let mut freed = 0u64;
    // Oldest first, which is the order both halves want.
    entries.sort_by_key(|(_, _, modified)| *modified);

    let keep = std::time::Duration::from_secs(u64::from(keep_days) * 24 * 60 * 60);
    let remove =
        |path: &Path, size: u64, total: &mut u64, freed: &mut u64| match std::fs::remove_file(path)
        {
            Ok(()) => {
                *total = total.saturating_sub(size);
                *freed += size;
            }
            Err(e) => tracing::debug!("sweeping {}: {e}", path.display()),
        };

    for (path, size, modified) in &entries {
        let age = now.duration_since(*modified).unwrap_or_default();
        // Zero days keeps nothing by age, which is what `keep_days = 0`
        // means everywhere else in this program.
        if keep_days == 0 || age > keep {
            remove(path, *size, &mut total, &mut freed);
        }
    }
    for (path, size, _) in &entries {
        if total <= max_bytes {
            break;
        }
        if path.exists() {
            remove(path, *size, &mut total, &mut freed);
        }
    }
    if freed > 0 {
        tracing::debug!("swept {freed} bytes out of the picture cache");
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::net::Replay;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let image =
            starkit::image::RgbaImage::from_pixel(w, h, starkit::image::Rgba([9, 9, 9, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        starkit::image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, starkit::image::ImageFormat::Png)
            .expect("encoding a fixture picture");
        out.into_inner()
    }

    /// The whole of the sizing rule: a picture that fits keeps its own size,
    /// one that does not is scaled by the tighter of the two ratios, and
    /// nothing is ever made bigger here.
    #[test]
    fn a_picture_is_scaled_down_to_its_box_and_never_up() {
        assert_eq!(size_for((64, 32), 640, 320), (64, 32), "it fits");
        assert_eq!(size_for((64, 32), 64, 32), (64, 32), "exactly");
        assert_eq!(size_for((640, 320), 320, 320), (320, 160), "by width");
        assert_eq!(size_for((320, 640), 320, 320), (160, 320), "by height");
        assert_eq!(size_for((1000, 10), 100, 100), (100, 1), "never to nothing");
        assert_eq!(size_for((64, 32), 0, 0), (64, 32), "an unmeasured box");
        assert_eq!(size_for((0, 0), 10, 10), (0, 0));
    }

    #[test]
    fn decoding_shrinks_to_the_box_and_remembers_the_size_on_the_wire() {
        let (image, natural) = decode(&png(64, 32), 640, 320).expect("a fixture picture");
        assert_eq!(natural, (64, 32));
        assert_eq!(
            image.dimensions(),
            (64, 32),
            "small pictures are left alone"
        );

        let (image, natural) = decode(&png(400, 200), 100, 100).expect("a fixture picture");
        assert_eq!(natural, (400, 200));
        assert_eq!(image.dimensions(), (100, 50));
    }

    /// A header that claims a picture far larger than the decoder's ceiling
    /// is refused before any pixels are read.
    #[test]
    fn something_absurd_is_refused_rather_than_allocated() {
        let mut bytes = png(2, 2);
        // The IHDR width and height are the four bytes each at offset 16 and
        // 20 of a PNG; a file that says it is 100000 wide is not one to
        // believe on the strength of its being eighty bytes long.
        bytes[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let err = decode(&bytes, 100, 100).expect_err("a huge picture was decoded");
        assert!(err.contains("could not be decoded"), "{err}");

        assert!(decode(b"not a picture at all", 10, 10).is_err());
        assert!(decode(&[], 10, 10).is_err());
    }

    /// The name is a hash and has nothing of the URL in it, which is what
    /// stops a URL with a slash or a `..` in it from naming a file outside
    /// the cache.
    #[test]
    fn a_cache_name_is_a_hash_and_an_extension() {
        let nasty = "https://e.org/../../../etc/passwd?a=b/c#frag";
        let name = cache_name(nasty, "png");
        assert!(name.ends_with(".png"), "{name}");
        assert_eq!(name.len(), 64 + 4, "{name}");
        assert!(name[..64].chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        assert_eq!(Path::new(&name).components().count(), 1, "{name}");

        // The same URL is the same file, and a different one is not -- down
        // to a query string, which on a picture is usually its size.
        assert_eq!(cache_name(nasty, "png"), cache_name(nasty, "png"));
        assert_ne!(
            cache_name("https://e.org/a.png?w=100", "png"),
            cache_name("https://e.org/a.png?w=200", "png")
        );
    }

    #[test]
    fn the_extension_is_the_servers_word_then_the_bytes_own() {
        assert_eq!(ext_for(Some("image/png"), b""), "png");
        assert_eq!(ext_for(Some("image/JPEG; charset=binary"), b""), "jpg");
        assert_eq!(ext_for(Some("image/webp"), b""), "webp");
        // A CDN that will not say gets read instead.
        assert_eq!(ext_for(None, &png(2, 2)), "png");
        assert_eq!(ext_for(Some("application/octet-stream"), &png(2, 2)), "png");
        assert_eq!(ext_for(None, b"neither"), "bin");
    }

    #[test]
    fn a_fetched_picture_is_cached_and_the_second_ask_reads_the_file() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let replay_dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(replay_dir.path().join("hero.png"), png(64, 32)).unwrap();
        std::fs::write(
            replay_dir.path().join("index.tsv"),
            "https://e.org/hero.png\thero.png\n",
        )
        .unwrap();
        let http = Replay::open(replay_dir.path()).expect("a replay directory");

        let got =
            fetch(&http, Some(dir.path()), "https://e.org/hero.png", 640, 320).expect("a picture");
        assert_eq!(got.natural, (64, 32));
        let path = got.path.expect("it was cached");
        assert!(path.is_file());
        assert_eq!(path.parent(), Some(dir.path()));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            png(64, 32),
            "the file is what arrived, not what was drawn"
        );

        // Now take the answer away: the second fetch is the file.
        std::fs::remove_file(replay_dir.path().join("hero.png")).unwrap();
        let again = fetch(&http, Some(dir.path()), "https://e.org/hero.png", 640, 320)
            .expect("the cached picture");
        assert_eq!(again.path.as_deref(), Some(path.as_path()));
        assert_eq!(again.natural, (64, 32));

        // And with nowhere to cache, a picture still arrives.
        std::fs::write(replay_dir.path().join("hero.png"), png(64, 32)).unwrap();
        let http = Replay::open(replay_dir.path()).expect("a replay directory");
        let uncached = fetch(&http, None, "https://e.org/hero.png", 640, 320).expect("a picture");
        assert_eq!(uncached.path, None);
    }

    #[test]
    fn a_picture_nothing_answers_for_says_so() {
        let replay_dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(replay_dir.path().join("index.tsv"), "").unwrap();
        let http = Replay::open(replay_dir.path()).expect("a replay directory");
        assert!(fetch(&http, None, "https://e.org/missing.png", 10, 10).is_err());
        assert!(fetch(&http, None, "not a url", 10, 10).is_err());
    }

    proptest::proptest! {
        /// A picture URL is a string out of somebody else's page. Whatever
        /// it is, the name it produces is one path component under the cache
        /// directory and nothing else -- which is the whole of why the name
        /// is a hash.
        #[test]
        fn any_url_names_one_file_inside_the_cache(url: String, bytes: Vec<u8>) {
            let dir = Path::new("/cache/pictures");
            for extension in EXTENSIONS {
                let name = cache_name(&url, extension);
                proptest::prop_assert_eq!(Path::new(&name).components().count(), 1, "{}", name);
                let path = dir.join(&name);
                proptest::prop_assert_eq!(path.parent(), Some(dir));
            }
            // And the extension chosen for any bytes at all is one of the
            // names a lookup will go on to probe.
            let extension = ext_for(None, &bytes);
            proptest::prop_assert!(EXTENSIONS.contains(&extension), "{}", extension);
        }
    }

    /// Both halves of the sweep: the old go first, and then the oldest of
    /// what is left until the directory is under its ceiling.
    #[test]
    fn the_sweep_takes_the_old_and_then_the_oldest() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60 * 24 * 60 * 60);
        let day = std::time::Duration::from_secs(24 * 60 * 60);

        let write = |name: &str, size: usize, days_ago: u64| {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![b'x'; size]).unwrap();
            let file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.set_modified(now - day * days_ago as u32).unwrap();
            path
        };
        let ancient = write("ancient.png", 10, 40);
        let old = write("old.png", 100, 20);
        let recent = write("recent.png", 100, 1);

        // Nothing to do: against a sixty-day ceiling it is all young enough,
        // and it all fits.
        assert_eq!(sweep(dir.path(), 1024, 60, now), 0);
        assert!(ancient.exists() && old.exists() && recent.exists());

        // Forty days old, against a thirty-day ceiling.
        assert_eq!(sweep(dir.path(), 1024, 30, now), 10);
        assert!(!ancient.exists());
        assert!(old.exists() && recent.exists());

        // Both young enough, and together over the size ceiling: the older
        // of the two goes.
        assert_eq!(sweep(dir.path(), 150, 30, now), 100);
        assert!(!old.exists());
        assert!(recent.exists(), "the one looked at yesterday stayed");

        // A directory that is not there is not an error.
        assert_eq!(sweep(&dir.path().join("nothing"), 1, 1, now), 0);
    }
}
