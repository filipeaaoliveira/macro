//! A GitHub stand-in for tests, here and in crates that call GitHub through
//! this one: an HTTP server that answers with scripted responses.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

/// What a [`MockServer`] answers one request with.
pub struct MockResponse {
    /// The status line's code and reason, such as `200 OK`.
    pub status: &'static str,
    /// The JSON body.
    pub body: String,
}

/// A local HTTP server that answers each request in turn with a scripted
/// response, and keeps the requests it got.
pub struct MockServer {
    /// Its address, to point a client at.
    pub base_url: String,
    requests: Receiver<String>,
    handle: JoinHandle<()>,
}

impl MockServer {
    /// Answers one request with `response`.
    pub fn start(response: MockResponse) -> Self {
        Self::start_many(vec![response])
    }

    /// Answers one connection per response, in order.
    pub fn start_many(responses: Vec<MockResponse>) -> Self {
        Self::start_with_headers(
            responses
                .into_iter()
                .map(|response| (response, ""))
                .collect(),
        )
    }

    /// Answers one connection per response, in order, each with its extra
    /// header lines (`Name: value\r\n` each).
    pub fn start_with_headers(responses: Vec<(MockResponse, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (request_sender, requests) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            for (response, headers) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                request_sender.send(request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                )
                .unwrap();
            }
        });

        Self {
            base_url,
            requests,
            handle,
        }
    }

    /// The one request it got, once answered.
    pub fn finish(self) -> String {
        self.finish_all().remove(0)
    }

    /// Every request it got, in order, once all are answered.
    pub fn finish_all(self) -> Vec<String> {
        self.handle.join().unwrap();
        self.requests.try_iter().collect()
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];

    loop {
        let bytes_read = stream.read(&mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..bytes_read]);

        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }

    String::from_utf8(request).unwrap()
}
