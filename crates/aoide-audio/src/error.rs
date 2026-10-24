use std::fmt;

/// What can go wrong between a source and the speaker. Messages never carry a
/// stream's URL, which holds its token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file or stream could not be opened.
    Open(String),
    /// Not a format or codec this build decodes.
    Unsupported(String),
    Decode(String),
    Seek(String),
    Resample(String),
    /// No output device, or it refused the stream.
    Output(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Open(m) => write!(f, "could not open the track: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported format: {m}"),
            Error::Decode(m) => write!(f, "decoding failed: {m}"),
            Error::Seek(m) => write!(f, "seeking failed: {m}"),
            Error::Resample(m) => write!(f, "resampling failed: {m}"),
            Error::Output(m) => write!(f, "audio output: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
