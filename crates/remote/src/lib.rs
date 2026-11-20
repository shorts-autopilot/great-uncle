//! OSC and browser remote for the OpenLight desk.
//!
//! The remote never owns lighting state. OSC messages become ordinary command
//! lines and the web command endpoint runs `Engine::execute` under the same
//! mutex the console uses. A bounded channel serialises every remote write, so
//! a flood of packets cannot hold the engine lock or starve the output tick.

pub mod http;
pub mod mapping;
pub mod osc;
pub mod token;

use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use engine::{Engine, OutputStatus, RemoteStatus};
use mapping::{RemoteCommand, TimecodeValue};

/// How many remote commands can wait for the engine. The network threads use
/// `try_send`, so a full queue is a dropped command, never a blocked socket.
pub const DISPATCH_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    pub osc_port: u16,
    pub http_port: u16,
    pub bind: IpAddr,
    pub token_path: PathBuf,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            osc_port: 9000,
            http_port: 8080,
            bind: IpAddr::from([127, 0, 0, 1]),
            token_path: token::default_path(),
        }
    }
}

#[derive(Debug)]
pub struct CommandReply {
    pub lines: Vec<String>,
    pub error: Option<String>,
}

/// One item for the single dispatch thread.
pub struct DispatchRequest {
    pub command: RemoteCommand,
    pub reply: Option<SyncSender<CommandReply>>,
}

pub struct Remote {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    osc_addr: SocketAddr,
    http_addr: SocketAddr,
    token_path: PathBuf,
}

impl Remote {
    pub fn start(engine: Arc<Mutex<Engine>>, config: RemoteConfig) -> Result<Self, String> {
        let token = token::load_or_create(&config.token_path)
            .map_err(|error| format!("cannot read remote token: {error}"))?;
        let socket = UdpSocket::bind((config.bind, config.osc_port))
            .map_err(|error| format!("cannot bind OSC port {}: {error}", config.osc_port))?;
        let osc_addr = socket
            .local_addr()
            .map_err(|error| format!("cannot read OSC address: {error}"))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|error| format!("cannot configure OSC socket: {error}"))?;
        let listener = TcpListener::bind((config.bind, config.http_port))
            .map_err(|error| format!("cannot bind remote port {}: {error}", config.http_port))?;
        let http_addr = listener
            .local_addr()
            .map_err(|error| format!("cannot read remote address: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("cannot configure remote listener: {error}"))?;

        let status = engine::output::lock(&engine).status.clone();
        {
            let mut status = engine::output::lock(&status);
            status.remote = Some(RemoteStatus {
                osc_port: osc_addr.port(),
                http_port: http_addr.port(),
                bind: config.bind.to_string(),
                ..RemoteStatus::default()
            });
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(DISPATCH_CAPACITY);
        let worker = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("openlight-remote-dispatch".into())
                .spawn(move || dispatch_loop(engine, receiver, stop))
                .map_err(|error| format!("cannot start remote dispatch: {error}"))?
        };

        let osc_thread = {
            let socket = socket;
            let sender = sender.clone();
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            thread::Builder::new()
                .name("openlight-osc".into())
                .spawn(move || osc_loop(socket, sender, stop, status))
                .map_err(|error| format!("cannot start OSC listener: {error}"))?
        };
        let http_thread = {
            let sender = sender;
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            let token = Arc::new(token);
            thread::Builder::new()
                .name("openlight-remote-http".into())
                .spawn(move || {
                    http::serve(listener, engine, sender, token, status, stop);
                })
                .map_err(|error| format!("cannot start remote server: {error}"))?
        };

        Ok(Self {
            stop,
            threads: vec![worker, osc_thread, http_thread],
            osc_addr,
            http_addr,
            token_path: config.token_path,
        })
    }

    /// The address the web page is served on. Port 0 in the config is replaced
    /// with the actual bound port.
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn osc_addr(&self) -> SocketAddr {
        self.osc_addr
    }

    pub fn token_path(&self) -> &std::path::Path {
        &self.token_path
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn dispatch_loop(
    engine: Arc<Mutex<Engine>>,
    receiver: Receiver<DispatchRequest>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let request = match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut engine = engine::output::lock(&engine);
        let reply = match request.command {
            RemoteCommand::Line(line) => match engine.execute(&line) {
                Ok(outcome) => CommandReply {
                    lines: outcome.lines,
                    error: None,
                },
                Err(error) => CommandReply {
                    lines: Vec::new(),
                    error: Some(error.to_string()),
                },
            },
            RemoteCommand::Timecode(TimecodeValue::Seconds(seconds)) => {
                engine.feed_osc_timecode_seconds(seconds);
                CommandReply {
                    lines: Vec::new(),
                    error: None,
                }
            }
            RemoteCommand::Timecode(TimecodeValue::Text(text)) => {
                engine.feed_osc_timecode_text(&text);
                CommandReply {
                    lines: Vec::new(),
                    error: None,
                }
            }
        };
        if let Some(reply_sender) = request.reply {
            let _ = reply_sender.send(reply);
        }
    }
}

fn osc_loop(
    socket: UdpSocket,
    sender: SyncSender<DispatchRequest>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<OutputStatus>>,
) {
    let mut buffer = [0u8; 65_536];
    while !stop.load(Ordering::Relaxed) {
        let (length, _) = match socket.recv_from(&mut buffer) {
            Ok(packet) => packet,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        };
        let (mut accepted, mut unmatched, mut dropped) = (0u64, 0u64, 0u64);
        match osc::decode(&buffer[..length]) {
            Ok(messages) => {
                for message in messages {
                    let Some(command) = mapping::command(&message) else {
                        unmatched += 1;
                        continue;
                    };
                    accepted += 1;
                    match sender.try_send(DispatchRequest {
                        command,
                        reply: None,
                    }) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(_)) => dropped += 1,
                        Err(mpsc::TrySendError::Disconnected(_)) => return,
                    }
                }
            }
            Err(_) => unmatched += 1,
        }
        if accepted + unmatched + dropped > 0 {
            let mut status = engine::output::lock(&status);
            if let Some(remote) = status.remote.as_mut() {
                remote.osc_accepted += accepted;
                remote.osc_unmatched += unmatched;
                remote.dropped += dropped;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::mpsc;
    use std::time::Instant;

    fn test_config(directory: &std::path::Path) -> RemoteConfig {
        RemoteConfig {
            osc_port: 0,
            http_port: 0,
            bind: IpAddr::from([127, 0, 0, 1]),
            token_path: directory.join("remote.token"),
        }
    }

    fn test_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "openlight-remote-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn osc_commands_reach_the_engine_and_counters_are_live() {
        let directory = test_directory("osc");
        let mut engine = Engine::empty();
        engine
            .execute("Patch Fixture 1 At 1.001 Type \"OpenLight Demo Wash\"")
            .ok();
        let engine = Arc::new(Mutex::new(engine));
        let remote = Remote::start(Arc::clone(&engine), test_config(&directory)).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let packet = osc::encode(&osc::OscMessage {
            address: "/openlight/grand".into(),
            values: vec![osc::OscValue::Float(0.5)],
        });
        socket.send_to(&packet, remote.osc_addr()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while engine::output::lock(&engine).desk.masters.grand != 0.5 {
            assert!(Instant::now() < deadline, "OSC command did not arrive");
            thread::sleep(Duration::from_millis(10));
        }
        {
            let status = engine::output::lock(&engine);
            let remote_status = status.status.lock().unwrap().remote.clone().unwrap();
            assert_eq!(remote_status.osc_accepted, 1);
            assert_eq!(remote_status.osc_unmatched, 0);
        }
        let report = engine::output::lock(&engine)
            .execute("Status")
            .unwrap()
            .lines
            .join("\n");
        assert!(report.contains("Remote: osc"), "{report}");
        remote.stop();
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn osc_positions_feed_an_active_osc_timecode_source() {
        let directory = test_directory("osc-timecode");
        let mut engine = Engine::empty();
        engine
            .execute("Store Timecode 1 Fps 25 Source Osc")
            .unwrap();
        engine.execute("Timecode 1 Start").unwrap();
        let engine = Arc::new(Mutex::new(engine));
        let remote = Remote::start(Arc::clone(&engine), test_config(&directory)).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        for (index, seconds) in [0.0f32, 0.5].into_iter().enumerate() {
            let packet = osc::encode(&osc::OscMessage {
                address: "/openlight/timecode".into(),
                values: vec![osc::OscValue::Float(seconds)],
            });
            socket.send_to(&packet, remote.osc_addr()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while {
                let status = engine::output::lock(&engine);
                let remote_status = status.status.lock().unwrap().remote.clone().unwrap();
                remote_status.osc_accepted < index as u64 + 1
            } {
                assert!(Instant::now() < deadline, "OSC timecode did not arrive");
                thread::sleep(Duration::from_millis(10));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while engine::output::lock(&engine).timecode.position() != 12 {
            assert!(Instant::now() < deadline, "OSC timecode was not applied");
            thread::sleep(Duration::from_millis(10));
        }
        remote.stop();
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn five_hundred_queued_commands_are_drained_in_order() {
        let engine = Arc::new(Mutex::new(Engine::empty()));
        let (sender, receiver) = mpsc::sync_channel(DISPATCH_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            thread::spawn(move || dispatch_loop(engine, receiver, stop))
        };
        let started = Instant::now();
        let (done_sender, done_receiver) = mpsc::sync_channel(1);
        for index in 0..500 {
            sender
                .send(DispatchRequest {
                    command: RemoteCommand::Line(if index % 2 == 0 {
                        "Master Grand At 25".into()
                    } else {
                        "Master Grand At 75".into()
                    }),
                    reply: (index == 499).then(|| done_sender.clone()),
                })
                .unwrap();
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the bounded queue blocked its sender"
        );
        done_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the last command did not run");
        drop(sender);
        stop.store(true, Ordering::SeqCst);
        worker.join().unwrap();
        assert_eq!(engine::output::lock(&engine).desk.masters.grand, 0.75);
    }

    #[test]
    fn a_port_already_in_use_is_a_startup_error() {
        let directory = test_directory("busy");
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let mut config = test_config(&directory);
        config.http_port = port;
        let error = match Remote::start(Arc::new(Mutex::new(Engine::empty())), config) {
            Ok(remote) => {
                remote.stop();
                panic!("the occupied port was accepted")
            }
            Err(error) => error,
        };
        assert!(error.contains("remote port"), "{error}");
        assert!(error.contains(&port.to_string()), "{error}");
        drop(occupied);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_full_dispatch_channel_drops_without_blocking() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(DispatchRequest {
                command: RemoteCommand::Line("Status".into()),
                reply: None,
            })
            .unwrap();
        assert!(sender
            .try_send(DispatchRequest {
                command: RemoteCommand::Line("Status".into()),
                reply: None,
            })
            .is_err());
        drop(receiver);
    }
}
