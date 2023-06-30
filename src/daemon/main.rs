use async_std::os::unix::net::{UnixListener, UnixStream};
use native_messaging_proxy::common::{InitMessage, send_msgs_task, recv_msgs_task, write_io_task, read_io_task};
use async_std::{prelude::*, task, process::Command, io, self, sync::Mutex};
use std::os::fd::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::{io::Cursor, fs, collections::HashMap, sync::OnceLock, pin::Pin, process::Stdio, net::Shutdown};
use byteorder::{NativeEndian, ReadBytesExt};

use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct Extension {
    name: String,
    description: String,
    path: String,
    #[serde(rename = "type")] type_: String,
}

static EXTENSIONS: OnceLock<HashMap<String, Extension>> = OnceLock::new();

#[async_std::main]
async fn main() {
    pretty_env_logger::init();

    let mut args = std::env::args_os();
    let _program_name = args.next().expect("unknown program name");

    let mut extensions = HashMap::new();

    let paths = fs::read_dir("/usr/lib/mozilla/native-messaging-hosts/").unwrap();
    for path in paths {
        let path = path.unwrap().path();

        // FIXME
        if path.extension().is_none() { continue; }
        if path.extension().unwrap() != "json" { continue; }

        let f = fs::File::open(path).unwrap();

        if let Ok(extension) = serde_json::from_reader::<_, Extension>(f) {
            extensions.insert(extension.name.clone(), extension);
        }
    }
    EXTENSIONS.set(extensions).unwrap();
    
    let fds: i32 = std::env::var("LISTEN_FDS").unwrap().parse().unwrap();
    let listener = unsafe { UnixListener::from_raw_fd(fds + 2) };
    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        let stream = stream.expect("No stream");
        log::info!("Accepting from: {:?}", stream.peer_addr().expect("No peer addr"));
        let _handle = task::spawn(handle_connection_and_disconnect(stream));
    }
}

async fn read_incoming_msg<'a>(stream: &mut Pin<Box<impl io::Read + 'a>>) -> Result<Vec<u8>, std::io::Error> {
    let mut size_buf = [0u8; 2];
    let mut data_buf = vec![];
    stream.read_exact(&mut size_buf).await?;
    let size: usize = Cursor::new(&size_buf).read_u16::<NativeEndian>().unwrap().into();
    data_buf.reserve_exact(size);
    stream.take(size as u64).read_to_end(&mut data_buf).await?;
    Ok(data_buf)
}

async fn send_stderr_string(stream: &mut Pin<Box<impl io::Write + Send>>, error: String) -> Result<(), std::io::Error> {
    let error_vec = error.into_bytes();
    stream.write(&[error_vec.len().try_into().unwrap(), 2u8]).await?;
    stream.write(&error_vec).await?;
    Ok(())
}

async fn handle_connection_and_disconnect(stream: UnixStream) {
    handle_connection(stream.clone()).await;
    stream.shutdown(Shutdown::Both).unwrap();
}

async fn handle_connection(stream: UnixStream) -> Result<(), ()> {
    let mut reader = stream.clone();
    let mut reader_pin = Box::pin(&mut reader);
    let mut writer = stream.clone();
    let mut writer_pin = Box::pin(&mut writer);

    let init_msg_vec = read_incoming_msg(&mut reader_pin).await.expect("Cannot read first message");
    let init_msg: InitMessage = serde_json::from_slice(&init_msg_vec[..]).unwrap();
    log::info!("{:?}", init_msg);
    let extension_name = init_msg.host_extension_name();
    
    match EXTENSIONS.get().unwrap().get(extension_name) {
        None => {
            send_stderr_string(&mut writer_pin, "No such extension on the host".into()).await.unwrap();
            Err(())
        }
        Some(extension) => {
            if extension.type_ == "stdio" {
                match Command::new(extension.path.to_string()).args(init_msg.args()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
                    Err(error) => {
                        send_stderr_string(&mut writer_pin, error.to_string()).await;  // FIXME
                        Err(())
                    },
                    Ok(mut child) => {
                        let child_stdin = child.stdin.take().unwrap();
                        let child_stdin_raw_fd = child_stdin.as_raw_fd();
                        let child_stdout = child.stdout.take().unwrap();
                        let child_stdout_raw_fd = child_stdout.as_raw_fd();
                        let child_stderr = child.stderr.take().unwrap();
                        let child_stderr_raw_fd = child_stderr.as_raw_fd();

                        let (mut in_s, in_r) = async_std::channel::unbounded();
                        let fut_recv = recv_msgs_task(&mut in_s, Box::pin(reader));
                        let mut stdin_writer: Pin<Box<dyn io::Write + Send>> = Box::pin(child_stdin);
                        let fut_write = write_io_task(in_r.clone(), Some(&mut stdin_writer), None, None);

                        let (mut out_s_1, out_r) = async_std::channel::unbounded();
                        let mut out_s_2 = out_s_1.clone();
                        let fut_read_stdout = read_io_task(&mut out_s_1, 1, Box::pin(child_stdout));
                        let fut_read_stderr = read_io_task(&mut out_s_2, 2, Box::pin(child_stderr));
                        let stream_writer_mutex: Mutex<Pin<Box<dyn io::Write + Send>>> = Mutex::new(Box::pin(writer));
                        let fut_send = send_msgs_task(out_r.clone(), &stream_writer_mutex);

                        let fut_out = fut_read_stdout.join(fut_read_stderr).join(fut_send);
                        let fut_in = fut_recv.join(fut_write);
                        async {
                            fut_in.await;
                            log::info!("Got EOF for IN");
                            out_r.close();
                            /*unsafe {
                                libc::close(child_stdin_raw_fd);
                            };*/
                        }.join(async {
                            fut_out.await;
                            log::info!("Got EOF for OUT");
                            in_r.close();
                            /*unsafe {
                                libc::close(child_stdout_raw_fd);
                                libc::close(child_stderr_raw_fd);
                            };*/
                        }).await;

                        child.kill();
                        stream.shutdown(Shutdown::Both);

                        Ok(())
                    }
                }
            } else {
                send_stderr_string(&mut writer_pin, "Only `stdio` extensions are supported".into()).await;  // FIXME
                Err(())
            }
        }
    }
}
