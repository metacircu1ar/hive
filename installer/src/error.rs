//! Owned installer failures with transparent observation of their native cause.
use std::{borrow::Cow, error::Error as NativeError, fmt};

#[derive(Debug)]
pub(crate) struct Error(Box<Kind>);
#[derive(Debug)]
struct Message(Cow<'static, str>);
impl fmt::Display for Message {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, out)
    }
}
impl NativeError for Message {}

// Every foreign conversion is concrete; internal errors are never stored erased.
macro_rules! failures {
    ($($variant:ident => $native:ty),+ $(,)?) => {
        #[derive(Debug)]
        enum Kind {
            $($variant($native),)+
            Message(Message),
        }
        $(impl From<$native> for Error {
            fn from(cause: $native) -> Self { Self(Box::new(Kind::$variant(cause))) }
        })+
        impl AsRef<dyn NativeError + Send + Sync> for Error {
            fn as_ref(&self) -> &(dyn NativeError + Send + Sync + 'static) {
                match self.0.as_ref() {
                    $(Kind::$variant(cause) => cause,)+
                    Kind::Message(message) => message,
                }
            }
        }
        impl Error {
            /// Only the std IO adapter needs an erased payload. Peel this owner to
            /// retain the original native object and chain.
            fn into_native(self) -> Box<dyn NativeError + Send + Sync> {
                match *self.0 {
                    $(Kind::$variant(cause) => Box::new(cause),)+
                    Kind::Message(message) => message.0.into_owned().into(),
                }
            }
        }
    };
}
failures! {
    Io => std::io::Error,
    Atomic => atomic_fs::Error,
    Json => serde_json::Error,
    Version => semver::Error,
    Integer => std::num::ParseIntError,
    IntegerRange => std::num::TryFromIntError,
    Utf8 => std::string::FromUtf8Error,
    Prefix => std::path::StripPrefixError,
    Input => hive_input::InputError,
    SharedRecord => hive_input::compatibility::record::Error,
    Metadata => hive_input::InvalidMetadata,
    LogInstall => hive_log::InstallError,
    Asset => crate::manifest::AssetError,
    Context => crate::publication::ContextFailure,
    Cleanup => crate::publication::CleanupFailure,
    SetupRetained => crate::setup::RetainedFailure,
}
impl Error {
    pub(crate) fn downcast_ref<E: NativeError + 'static>(&self) -> Option<&E> {
        self.as_ref().downcast_ref()
    }
    pub(crate) fn into_io_other(self) -> std::io::Error {
        std::io::Error::other(self.into_native())
    }
}
impl From<&'static str> for Error {
    fn from(message: &'static str) -> Self {
        Self(Box::new(Kind::Message(Message(Cow::Borrowed(message)))))
    }
}
impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(Box::new(Kind::Message(Message(Cow::Owned(message)))))
    }
}
impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), out)
    }
}
impl NativeError for Error {
    fn source(&self) -> Option<&(dyn NativeError + 'static)> {
        self.as_ref().source()
    }
}

#[cfg(test)]
mod tests;
