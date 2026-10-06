use std::{
    fmt,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use http_body_util::{BodyExt, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::{Frame, SizeHint};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A response body: either a single buffered chunk or a stream of chunks.
///
/// Usually you never build one yourself — handlers return strings, bytes,
/// [`Json`](crate::Json) and so on. Use [`Body::from_stream`] to send data as
/// it is produced, without holding all of it in memory.
pub struct Body(Inner);

enum Inner {
    /// Already in memory; `None` once sent.
    Full(Option<Bytes>),
    Stream(UnsyncBoxBody<Bytes, BoxError>),
}

impl Body {
    /// An empty body.
    pub fn empty() -> Self {
        Body(Inner::Full(None))
    }

    /// A body sent chunk by chunk as `stream` yields them. If the stream
    /// returns an error, the connection is closed.
    ///
    /// ```
    /// use rustyweb::Body;
    ///
    /// let chunks = futures_util::stream::iter(["hello, ", "world"].map(Ok::<_, std::io::Error>));
    /// let body = Body::from_stream(chunks);
    /// ```
    pub fn from_stream<S, T, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        T: Into<Bytes> + 'static,
        E: Into<BoxError> + 'static,
    {
        let frames = stream
            .map_ok(|chunk| Frame::data(chunk.into()))
            .map_err(Into::into);
        Body(Inner::Stream(StreamBody::new(frames).boxed_unsync()))
    }

    fn full(bytes: Bytes) -> Self {
        Body(Inner::Full((!bytes.is_empty()).then_some(bytes)))
    }

    /// The whole body, if it is a single buffered chunk (not a stream).
    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            Inner::Full(bytes) => Some(bytes.as_deref().unwrap_or_default()),
            Inner::Stream(_) => None,
        }
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body").finish_non_exhaustive()
    }
}

macro_rules! body_from {
    ($($ty:ty),*) => {$(
        impl From<$ty> for Body {
            fn from(value: $ty) -> Self {
                Body::full(Bytes::from(value))
            }
        }
    )*};
}
body_from!(Bytes, String, Vec<u8>, &'static str, &'static [u8]);

impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        match &mut self.0 {
            Inner::Full(bytes) => Poll::Ready(bytes.take().map(|b| Ok(Frame::data(b)))),
            Inner::Stream(stream) => Pin::new(stream).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.0 {
            Inner::Full(bytes) => bytes.is_none(),
            Inner::Stream(stream) => stream.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.0 {
            Inner::Full(bytes) => {
                SizeHint::with_exact(bytes.as_ref().map_or(0, |b| b.len() as u64))
            }
            Inner::Stream(stream) => stream.size_hint(),
        }
    }
}
