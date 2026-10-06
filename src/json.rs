use http::{HeaderValue, header};
use serde::Serialize;

use crate::{Body, Error, IntoResponse, Response};

/// A JSON response. Serialization failures become a `500` [`Error`].
///
/// To read a JSON request body, use [`Request::json`](crate::Request::json).
#[derive(Clone, Debug)]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(body) => {
                let mut res = Response::new(Body::from(body));
                res.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                res
            }
            Err(e) => {
                Error::internal(format!("failed to serialize JSON response: {e}")).into_response()
            }
        }
    }
}
