use std::io::ErrorKind;
use std::net::UdpSocket;
use std::process;
use std::thread;
use std::time::Instant;

use rouille::Server;
use rouille::{Request, Response};

use str0m::net::Receive;
use str0m::{Candidate, Input, Offer, Output, Rtc, RtcError};

fn init_log() {
    use std::env;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "trace");
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}

pub fn main() {
    init_log();

    let certificate = include_bytes!("cer.pem").to_vec();
    let private_key = include_bytes!("key.pem").to_vec();
    let server = Server::new_ssl("127.0.0.1:3000", web_request, certificate, private_key)
        .expect("starting the web server");
    println!("Listening on {:?}", server.server_addr().port());
    server.run();
}

// Handle a web request.
fn web_request(request: &Request) -> Response {
    if request.method() == "GET" {
        return Response::html(include_str!("http-post.html"));
    }

    // Expected POST SDP Offers.
    let mut data = request.data().expect("body to be available");

    let offer: Offer = serde_json::from_reader(&mut data).expect("serialized offer");
    let mut rtc = Rtc::new();

    // Spin up a UDP socket for the RTC
    let socket = UdpSocket::bind("127.0.0.1:0").expect("binding a random UDP port");
    let addr = socket.local_addr().expect("a local socket adddress");
    let candidate = Candidate::host(addr).expect("a host candidate");
    rtc.add_local_candidate(candidate);

    // Create an SDP Answer.
    let answer = rtc.accept_offer(offer).expect("offer to be accepted");

    // Launch WebRTC in separate thread.
    thread::spawn(|| {
        if let Err(e) = run(rtc, socket) {
            eprintln!("Exited: {:?}", e);
            process::exit(1);
        }
    });

    let body = serde_json::to_vec(&answer).expect("answer to serialize");

    Response::from_data("application/json", body)
}

fn run(mut rtc: Rtc, socket: UdpSocket) -> Result<(), RtcError> {
    // Buffer for incoming data.
    let mut buf = Vec::new();

    loop {
        // Poll output until we get a timeout. The timeout means we are either awaiting UDP socket input
        // or the timeout to happen.
        let timeout = match rtc.poll_output()? {
            Output::Timeout(v) => v,

            Output::Transmit(v) => {
                socket.send_to(&v.contents, v.destination)?;
                continue;
            }

            Output::Event(_v) => {
                //                println!("{:?}", v);
                continue;
            }
        };

        let timeout = timeout - Instant::now();

        // socket.set_read_timeout(Some(0)) is not ok
        if timeout.is_zero() {
            rtc.handle_input(Input::Timeout(Instant::now()))?;
            continue;
        }

        socket.set_read_timeout(Some(timeout))?;
        buf.resize(2000, 0);

        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                buf.truncate(n);
                Input::Receive(
                    Instant::now(),
                    Receive {
                        source,
                        destination: socket.local_addr().unwrap(),
                        contents: buf.as_slice().try_into()?,
                    },
                )
            }

            Err(e) => match e.kind() {
                // Expected error for set_read_timeout(). One for windows, one for the rest.
                ErrorKind::WouldBlock | ErrorKind::TimedOut => Input::Timeout(Instant::now()),
                _ => return Err(e.into()),
            },
        };

        rtc.handle_input(input)?;
    }
}