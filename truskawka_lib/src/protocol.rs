use ascii::{AsciiString, FromAsciiError};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::stream::StreamExt;
use tokio_util::codec::{Decoder, Encoder};

struct Request {
    strings: Vec<AsciiString>,
}

#[derive(thiserror::Error, Debug)]
enum InvalidRequestError {
    #[error("error with underlying IO operation")]
    IOError {
        #[from]
        source: std::io::Error,
    },
    #[error("failed loading string due to bad ascii encoding")]
    BadAsciiEncoding {
        #[from]
        source: FromAsciiError<BytesMut>,
    },
}

struct RequestCodec {}

impl RequestCodec {
    fn ready(src: &mut Bytes) -> bool {
        log::debug!("Checking request frame readiness");
        if src.remaining() < 4 {
            return false;
        }
        let n_strings = src.get_u32();
        for _ in 0..n_strings {
            if src.remaining() < 4 {
                return false;
            }
            let len = src.get_u32() as usize;
            if src.remaining() < len {
                return false;
            }
            src.advance(len);
        }
        log::debug!("Request frame ready");
        true
    }

    fn read_frame(src: &mut BytesMut) -> Result<Request, InvalidRequestError> {
        let n_strings = src.get_u32();
        let mut strings = Vec::new();
        for _ in 0..n_strings {
            let len = src.get_u32();
            let string = AsciiString::from_ascii(src.split_to(len as usize))
                .map_err(|e| InvalidRequestError::BadAsciiEncoding { source: e })?;
            strings.push(string);
        }
        Ok(Request { strings })
    }
}

impl Decoder for RequestCodec {
    type Item = Request;
    type Error = InvalidRequestError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if !Self::ready(&mut src.clone().freeze()) {
            return Ok(None);
        }
        Ok(Some(Self::read_frame(src)?))
    }
}

impl Encoder<Request> for RequestCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Request, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let strings_len = item
            .strings
            .iter()
            .fold(0, |acc, string| acc + string.len());
        let len = ((u32::BITS / 8) * 2) as usize + strings_len;
        dst.reserve(len);
        dst.put_u32(item.strings.len() as u32);
        for string in item.strings {
            dst.put_u32(string.len() as u32);
            dst.put(string.as_ref())
        }
        Ok(())
    }
}

enum ResponseStatusCode {
    Ok,
    Err,
    Nx,
}

struct Response {
    status_code: u32,
    data: AsciiString,
}

struct ResponseCodec {}

#[derive(thiserror::Error, Debug)]
enum InvalidResponseError {
    #[error("error with underlying IO operation")]
    IOError {
        #[from]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use futures::SinkExt;
    use tokio_util::codec::{FramedRead, FramedWrite};

    use super::*;

    fn example_request_bytes() -> BytesMut {
        let strings = [
            AsciiString::from_str("xyz").unwrap(),
            AsciiString::from_ascii("abcd").unwrap(),
        ];
        let mut buffer = BytesMut::with_capacity(4 + 4 + 3 + 4);
        buffer.put_u32(2);
        buffer.put_u32(strings[0].len() as u32);
        buffer.put(strings[0].as_ref());
        buffer.put_u32(strings[1].len() as u32);
        buffer.put(strings[1].as_ref());
        buffer
    }

    #[tokio::test]
    async fn test_decoding_correct_request_frame() {
        let buffer = example_request_bytes();
        let mut stream = FramedRead::new(&buffer[..], RequestCodec {});
        let request = stream.next().await.unwrap().unwrap();
        assert_eq!(request.strings.len(), 2);
        assert_eq!(request.strings[0].to_string(), "xyz");
        assert_eq!(request.strings[1].to_string(), "abcd");
    }

    #[tokio::test]
    async fn test_decoding_request_frame_with_excess_bytes() {
        let mut buffer = example_request_bytes();
        buffer.reserve(3);
        buffer.put([0 as u8; 3].as_slice());
        let mut stream = FramedRead::new(&buffer[..], RequestCodec {});
        let request = stream.next().await.unwrap().unwrap();
        assert_eq!(request.strings.len(), 2);
        assert_eq!(request.strings[0].to_string(), "xyz");
        assert_eq!(request.strings[1].to_string(), "abcd");
    }

    #[tokio::test]
    async fn test_encoding_request_frame() {
        let request = Request {
            strings: vec![
                AsciiString::from_str("xyz").unwrap(),
                AsciiString::from_str("abcd").unwrap(),
            ],
        };
        let mut sink = FramedWrite::new(Vec::new(), RequestCodec {});
        sink.send(request).await.unwrap();
        let serialized = sink.into_inner();
        assert_eq!(&serialized[..], example_request_bytes().as_ref());
    }
}
