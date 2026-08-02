//! Bounded gunzip and gzip for VGZ, shared by both VGM readers.
//!
//! A VGZ is just a gzipped VGM. Decompressing an untrusted stream has to be
//! bounded: the gzip trailer's ISIZE is attacker-controlled, so it is never
//! consulted. Instead the decompressor is capped at [`MAX_DECOMPRESSED`] bytes
//! by counting what comes out, and reports hitting the ceiling as an error
//! rather than allocating whatever the file implies. The cap is identical on
//! native and wasm32, so a file opens -- or is refused -- the same way
//! everywhere.

use std::io::Read;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::error::{Error, Result};

/// The most a VGZ may inflate to: 256 MiB, native and wasm32 alike.
///
/// This is a hostile-file ceiling, not a real-file limit -- the largest rips are
/// a few tens of MiB. It is necessary but not sufficient on its own: the command
/// index built over the decompressed body amplifies it about twelvefold, which
/// [`VgmStream::parse`](crate::vgm::stream::VgmStream::parse) bounds separately.
pub(crate) const MAX_DECOMPRESSED: usize = 256 * 1024 * 1024;

/// Inflates a gzip stream, refusing one that would exceed [`MAX_DECOMPRESSED`].
///
/// # Errors
/// If the gzip stream is corrupt, or inflates past the ceiling. The error is
/// "hit the ceiling", not "declared too big": the ISIZE trailer is never
/// trusted, so the read is stopped by counting bytes, not by reading the header.
pub(crate) fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    gunzip_capped(bytes, MAX_DECOMPRESSED)
}

/// [`gunzip`] with an explicit ceiling, so a test can prove the cap holds
/// without inflating hundreds of megabytes.
fn gunzip_capped(bytes: &[u8], cap: usize) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    // `take` stops the decoder one byte past the ceiling, so a stream that
    // exactly fills it still succeeds and only a genuine overrun trips the check.
    GzDecoder::new(bytes)
        .take(cap as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| Error::file(format!("Could not decompress the VGZ file: {error}")))?;
    if decoded.len() > cap {
        return Err(Error::file(format!(
            "the VGZ inflates past the {cap} byte ceiling"
        )));
    }
    Ok(decoded)
}

/// Compresses VGM bytes to a VGZ.
///
/// # Errors
/// If the compressor fails, which for an in-memory target it does not in
/// practice.
pub(crate) fn gzip(plain: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(plain)
        .and_then(|()| encoder.finish())
        .map_err(|error| Error::file(format!("Could not compress the VGZ file: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_inflating_past_the_ceiling_is_refused() {
        // A tiny gzip that inflates to more than a deliberately small cap.
        let bomb = gzip(&vec![0u8; 4096]).unwrap();
        assert!(bomb.len() < 1024, "zeros should compress far under the cap");
        let error = gunzip_capped(&bomb, 1024).unwrap_err();
        assert!(error.to_string().contains("ceiling"), "{error}");
    }

    #[test]
    fn a_stream_within_the_ceiling_is_accepted() {
        let ok = gzip(&vec![0x5Au8; 512]).unwrap();
        assert_eq!(gunzip_capped(&ok, 1024).unwrap(), vec![0x5Au8; 512]);
    }

    #[test]
    fn a_stream_exactly_at_the_ceiling_is_accepted() {
        let ok = gzip(&vec![0x11u8; 1024]).unwrap();
        assert_eq!(gunzip_capped(&ok, 1024).unwrap().len(), 1024);
    }

    #[test]
    fn a_corrupt_stream_is_an_error_not_a_panic() {
        assert!(gunzip(&[0x1F, 0x8B, 0x08, 0x00, 0xDE, 0xAD, 0xBE, 0xEF]).is_err());
    }
}
