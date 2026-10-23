use std::io;
use std::process::ExitStatus;

use thiserror::Error;
use wayland_client::globals::{BindError, GlobalError};
use wayland_client::ConnectError;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read the notulist config file")]
    Read(#[source] io::Error),
    #[error("cannot parse the notulist config file")]
    Parse(#[source] toml::de::Error),
    #[error("the config file given on the command line does not exist")]
    ExplicitFileMissing,
}

#[derive(Debug, Error)]
pub enum ImeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot reach the notulist session bus service")]
    Bus(#[from] zbus::Error),
    #[error("cannot connect to the Wayland compositor; is WAYLAND_DISPLAY set?")]
    WaylandConnect(#[from] ConnectError),
    #[error("the Wayland compositor does not offer the globals notulist-ime needs")]
    WaylandGlobals(#[from] GlobalError),
    #[error("the Wayland compositor is missing a required global")]
    WaylandBind(#[from] BindError),
    #[error("the Wayland event loop failed")]
    EventLoop(#[from] calloop::Error),
    #[error("cannot run dotool; is it installed?")]
    Dotool(#[source] io::Error),
    #[error("dotool exited with {0}")]
    DotoolExit(ExitStatus),
}

impl<T> From<calloop::InsertError<T>> for ImeError {
    fn from(error: calloop::InsertError<T>) -> Self {
        Self::EventLoop(error.into())
    }
}
