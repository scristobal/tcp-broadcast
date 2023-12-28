use clap::Parser;
use tokio::select;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, Result};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_util::bytes::BytesMut;

/// Simple TCP broadcaster, connects to a remote TCP host and broadcast to a local TCP socket
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Local host for clients to connect and get data pushed
    #[arg(short = 'h', long, default_value_t = String::from("localhost"), env = "HOST")]
    local_host: String,
    /// Local port for clients to connect and get data pushed
    #[arg(short = 'p', long, default_value_t = 8080u16, env = "PORT")]
    local_port: u16,

    /// Remote host to pull data from
    #[arg(short = 'r', long, env = "REMOTE_HOST")]
    remote_host: String,

    /// Remote port to pull data from
    #[arg(short = 'q', long, env = "REMOTE_PORT")]
    remote_port: u16,
}

const BUFFER_SIZE: usize = 1024;

async fn run(
    local_host: String,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
    cancel: CancellationToken,
) -> Result<()> {
    let source = format!("{remote_host}:{remote_port}")
        .parse::<SocketAddr>()
        .expect("Failed to parse remote host address");

    let source = TcpStream::connect(source)
        .await
        .expect("Failed to connect to remote host");

    let sinks: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(vec![]));

    let listener = format!("{local_host}:{local_port}")
        .parse::<SocketAddr>()
        .expect("Failed to parse local host address");

    let listener = TcpListener::bind(listener)
        .await
        .expect("Failed to bind TCP listener");

    let mut buffer = BytesMut::with_capacity(BUFFER_SIZE);
    let mut reader = BufReader::new(source);

    loop {
        let read = reader.read_buf(&mut buffer);

        tokio::select! {
            _ =  cancel.cancelled() => {
                break;
            }
            Ok(n) = read => {

                let mut sinks = sinks.lock().await;
                let mut failed_sink_indexes = vec![];

                for (index, sink) in  sinks.iter_mut().enumerate() {

                    let  mut data = buffer.split_to(n).freeze();
                    let mut writer = BufWriter::new(sink);

                    let mut m = 0;

                     while m < n {
                        m += match writer.write_buf(&mut data).await {
                            Err(e) => {
                                eprintln!("Error writing: {}", e);
                                failed_sink_indexes.push(index);
                                break;
                            },
                            Ok(0) => {
                                // Ok(0) means nothing was be written, so most likely the client socket disconnected and needs to be removed
                                failed_sink_indexes.push(index);
                                break;
                            },

                            Ok(m) => {
                                m
                            },
                        }
                    }

                }

                for index in failed_sink_indexes {
                    sinks.remove(index);
                }
            }
            Ok(( stream,_)) = listener.accept() => {
                let mut sinks = sinks.lock().await;
                sinks.push(stream);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        local_port,
        local_host,
        remote_port,
        remote_host,
    } = Args::parse();

    println!("running with passed args {:?}", Args::parse());

    let cancel = CancellationToken::new();

    tokio::spawn(run(
        local_host,
        local_port,
        remote_host,
        remote_port,
        cancel.clone(),
    ));

    tokio::signal::ctrl_c().await?;

    cancel.cancel();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn run_test() {
        // create a tests server

        let local_host = "127.0.0.1".to_string();
        let local_port = 8082;

        let remote_host = "127.0.0.1".to_string();
        let remote_port = 9092;

        let remote_addr = format!("{}:{}", remote_host, remote_port);

        let remote = TcpListener::bind(remote_addr.clone()).await.unwrap();

        let mut sink_1 = TcpStream::connect(remote_addr.clone()).await.unwrap();
        let mut sink_2 = TcpStream::connect(remote_addr.clone()).await.unwrap();

        let cancel = CancellationToken::new();

        tokio::spawn(run(
            local_host,
            local_port,
            remote_host,
            remote_port,
            cancel.clone(),
        ));

        let test_data = vec![1, 2, 3];

        tokio::spawn({
            let data = test_data.clone();
            async move {
                // Read data from the stream
                let mut buffer = [0u8; BUFFER_SIZE];
                let n = sink_1.read(&mut buffer).await.unwrap();

                assert_eq!(buffer[..n].to_vec(), data);
            }
        });

        tokio::spawn({
            let data = test_data.clone();

            async move {
                // Read data from the stream
                let mut buffer = [0u8; BUFFER_SIZE];
                let n = sink_2.read(&mut buffer).await.unwrap();

                assert_eq!(buffer[..n].to_vec(), data);
            }
        });

        let mut remote_stream = remote.accept().await.unwrap().0;
        let r = remote_stream.write(&test_data).await.unwrap();

        cancel.cancel();

        assert_eq!(r, test_data.len());
    }
}
