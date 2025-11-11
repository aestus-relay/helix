use axum::response::{IntoResponse, Response};
use http::{HeaderMap, header::{ACCEPT, CONTENT_TYPE}};
use serde::Serialize;
use ssz::Encode;
use std::time::Instant;

use helix_common::metrics::PROPOSER_ENCODE_LATENCY;

use super::error::ProposerApiError;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Encoding {
    Json,
    Ssz,
}

/// Encodes proposer API responses (SSZ or JSON based on Accept header)
/// 
/// Currently, SSZ is only enabled for requests with
/// the X-SSZ-Test header. In prod, the test header check
/// should be removed (marked with TODO comments).
pub struct ProposerResponseEncoder {
    encoding: Encoding,
    is_test_request: bool,
}

impl ProposerResponseEncoder {
    /// Create encoder from HTTP headers
    /// 
    /// Detects preferred encoding from Accept header:
    /// - Accept contains `application/octet-stream` + X-SSZ-Test: true → SSZ
    /// - Accept contains `application/octet-stream` (no test header) → JSON (during testing)
    /// - Accept: application/json → JSON
    /// - Accept: */* → JSON (default)
    /// - (no header) → JSON (default)
    pub fn from_headers(headers: &HeaderMap) -> Self {
        const SSZ_ACCEPT: &str = "application/octet-stream";
        const SSZ_TEST_HEADER: &str = "x-ssz-test";

        // Check if this is a test request
        let is_test_request = headers
            .get(SSZ_TEST_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let encoding = headers
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|accept_str| {
                if accept_str.contains(SSZ_ACCEPT) {
                    // TODO(SSZ-GA): Remove this test header check for general availability
                    // For GA, just use: Encoding::Ssz
                    if is_test_request {
                        Encoding::Ssz
                    } else {
                        tracing::debug!("SSZ accepted without X-SSZ-Test header, using JSON");
                        Encoding::Json
                    }
                } else {
                    Encoding::Json
                }
            })
            .unwrap_or(Encoding::Json); // Default to JSON for backward compatibility

        Self {
            encoding,
            is_test_request,
        }
    }

    /// Encode data into HTTP response
    /// 
    /// T must implement both ssz::Encode and serde::Serialize
    /// to support both encoding formats.
    pub fn encode<T: Encode + Serialize>(
        &self,
        data: &T,
    ) -> Result<Response, ProposerApiError> {
        let start = Instant::now();

        let response = match self.encoding {
            Encoding::Ssz => {
                let bytes = data.as_ssz_bytes();
                Response::builder()
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(axum::body::Body::from(bytes))
                    .map_err(|_| ProposerApiError::InternalServerError)?
            }
            Encoding::Json => axum::Json(data).into_response(),
        };

        let latency = start.elapsed();
        PROPOSER_ENCODE_LATENCY
            .with_label_values(&["unknown", self.encoding_label()])
            .observe(latency.as_micros() as f64);

        Ok(response)
    }

    /// Get encoding label for metrics
    pub fn encoding_label(&self) -> &'static str {
        match self.encoding {
            Encoding::Ssz => "ssz",
            Encoding::Json => "json",
        }
    }

    /// Check if using SSZ encoding
    pub fn is_ssz(&self) -> bool {
        matches!(self.encoding, Encoding::Ssz)
    }

    /// Check if using JSON encoding
    pub fn is_json(&self) -> bool {
        matches!(self.encoding, Encoding::Json)
    }

    /// Check if this is a test request (has X-SSZ-Test header)
    pub fn is_test(&self) -> bool {
        self.is_test_request
    }

    /// Get test label for metrics
    pub fn test_label(&self) -> &'static str {
        if self.is_test_request {
            "test"
        } else {
            "production"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_types::{BuilderBid, BidTrace};
    use ssz::Encode;

    fn ssz_accept_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/octet-stream".parse().unwrap());
        headers
    }

    fn ssz_accept_with_test() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/octet-stream".parse().unwrap());
        headers.insert("x-ssz-test", "true".parse().unwrap());
        headers
    }

    fn json_accept_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/json".parse().unwrap());
        headers
    }

    #[test]
    fn test_encoder_ssz_with_test_header() {
        let headers = ssz_accept_with_test();
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        assert!(encoder.is_ssz());
        assert!(encoder.is_test());
        assert_eq!(encoder.encoding_label(), "ssz");
        assert_eq!(encoder.test_label(), "test");
    }

    #[test]
    fn test_encoder_ssz_without_test_header() {
        let headers = ssz_accept_headers();
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        // During testing phase, should fallback to JSON without test header
        assert!(encoder.is_json());
        assert!(!encoder.is_test());
        assert_eq!(encoder.encoding_label(), "json");
    }

    #[test]
    fn test_encoder_json_accept() {
        let headers = json_accept_headers();
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        assert!(encoder.is_json());
        assert!(!encoder.is_test());
        assert_eq!(encoder.encoding_label(), "json");
    }

    #[test]
    fn test_encoder_no_accept() {
        let headers = HeaderMap::new();
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        assert!(encoder.is_json());
        assert_eq!(encoder.encoding_label(), "json");
    }

    #[test]
    fn test_encoder_wildcard_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "*/*".parse().unwrap());
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        assert!(encoder.is_json()); // Default to JSON for wildcard
    }

    #[test]
    fn test_encoder_multiple_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            "application/json, application/octet-stream".parse().unwrap(),
        );
        headers.insert("x-ssz-test", "true".parse().unwrap());
        let encoder = ProposerResponseEncoder::from_headers(&headers);

        assert!(encoder.is_ssz()); // Should detect SSZ in list with test header
    }

    #[test]
    fn test_encoder_test_header_variations() {
        // Test "1" as test value
        let mut headers = ssz_accept_headers();
        headers.insert("x-ssz-test", "1".parse().unwrap());
        let encoder = ProposerResponseEncoder::from_headers(&headers);
        assert!(encoder.is_ssz());
        assert!(encoder.is_test());

        // Test "0" as non-test value
        let mut headers = ssz_accept_headers();
        headers.insert("x-ssz-test", "0".parse().unwrap());
        let encoder = ProposerResponseEncoder::from_headers(&headers);
        assert!(encoder.is_json()); // Fallback
    }

    #[test]
    fn test_encode_ssz_sets_content_type() {
        use helix_types::TestRandomSeed;
        let encoder = ProposerResponseEncoder {
            encoding: Encoding::Ssz,
            is_test_request: true,
        };
        let data = BidTrace::test_random();
        let response = encoder.encode(&data).unwrap();

        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_encode_json_sets_content_type() {
        use helix_types::TestRandomSeed;
        let encoder = ProposerResponseEncoder {
            encoding: Encoding::Json,
            is_test_request: false,
        };
        let data = BidTrace::test_random();
        let response = encoder.encode(&data).unwrap();

        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[test]
    fn test_encode_ssz_response() {
        use helix_types::TestRandomSeed;
        let encoder = ProposerResponseEncoder {
            encoding: Encoding::Ssz,
            is_test_request: true,
        };
        let data = BuilderBid::test_random();
        let response = encoder.encode(&data).unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_encode_json_response() {
        use helix_types::TestRandomSeed;
        let encoder = ProposerResponseEncoder {
            encoding: Encoding::Json,
            is_test_request: false,
        };
        let data = BuilderBid::test_random();
        let response = encoder.encode(&data).unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
}

