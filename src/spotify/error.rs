use std::error;
use std::fmt::Display;

#[derive(Debug)]
pub enum APILoginError {
    OAuth,
    Token,
    Credentials,
}

impl error::Error for APILoginError {}

impl Display for APILoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            APILoginError::OAuth => write!(f, "Failed to load OAuth data from .env file"),
            APILoginError::Token => write!(f, "Failed to parse response token"),
            APILoginError::Credentials => write!(f, "Failed to load credentials from .env file"),
        }
    }
}

#[derive(Debug)]
pub enum WorkerError {
    NoAPIClient,
    NoSpotifyPlayer,
    NoSpotifySession,

    BadSpotifyId,
}

impl error::Error for WorkerError {}

impl Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::NoAPIClient => write!(f, "A Spotify API client wasn't created."),
            WorkerError::NoSpotifyPlayer => write!(f, "A Spotify player wasn't created."),
            WorkerError::NoSpotifySession => write!(f, "A Spotify session wasn't created."),

            WorkerError::BadSpotifyId => write!(f, "An invalid Spotify ID was provided.")
        }
    }
}
