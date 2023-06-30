use async_std::fs::File;
use native_messaging_proxy::common::{InitMessage, recv_msgs_task, write_io_task, read_io_task, send_msgs_task};

use async_std::{prelude::*, io, self, sync::Mutex, os::unix::net::UnixStream};
use std::{pin::Pin, net::Shutdown, path::Path};
use std::os::unix::io::FromRawFd;
use byteorder::{NativeEndian, WriteBytesExt};

#[async_std::main]
async fn main() {
    pretty_env_logger::init();

    let mut args = std::env::args_os();
    let program_name = args.next().expect("unknown program name");
    let program_filename = Path::new(program_name.as_os_str()).file_name().unwrap();

    let nmp_sock_path = Path::new("/run/user/1000/nmp.sock");
    let stream = UnixStream::connect(nmp_sock_path).await.expect("Could not connect to daemon socket");

    let reader = stream.clone();
    let mut writer = stream.clone();
    let mut writer_pin = Box::pin(&mut writer);

    let init_msg = InitMessage::new(program_filename.to_os_string().into_string().unwrap(), Vec::from_iter(args.map(|arg| arg.into_string().unwrap_or_default())));
    let init_msg_vec = serde_json::to_vec(&init_msg).unwrap();

    let mut size_vec = Vec::with_capacity(2);
    size_vec.write_u16::<NativeEndian>(init_msg_vec.len().try_into().unwrap()).unwrap();
    writer_pin.write(&size_vec).await.unwrap();
    writer_pin.write(&init_msg_vec).await.unwrap();

    let stdin = unsafe { File::from_raw_fd(0) };
    let stdout = unsafe { File::from_raw_fd(1) };
    let stderr = unsafe { File::from_raw_fd(2) };

    let (mut in_s, in_r) = async_std::channel::unbounded();
    let fut_recv = recv_msgs_task(&mut in_s, Box::pin(reader));
    let mut stdout_writer: Pin<Box<dyn io::Write + Send>> = Box::pin(stdout);
    let mut stderr_writer: Pin<Box<dyn io::Write + Send>> = Box::pin(stderr);
    let fut_write = write_io_task(in_r.clone(), None, Some(&mut stdout_writer), Some(&mut stderr_writer));

    let (mut out_s, out_r) = async_std::channel::unbounded();
    let fut_read = read_io_task(&mut out_s, 0, Box::pin(stdin));
    let stream_writer_mutex: Mutex<Pin<Box<dyn io::Write + Send>>> = Mutex::new(writer_pin);
    let fut_send = send_msgs_task(out_r.clone(), &stream_writer_mutex);

    let fut_in = fut_read.join(fut_send);
    let fut_out = fut_recv.join(fut_write);
    async {
        fut_in.await;
        log::info!("Got EOF for IN");
        out_r.close();
        /*unsafe {
            libc::close(1);
            libc::close(2);
        };*/
    }.join(async {
        fut_out.await;
        log::info!("Got EOF for OUT");
        in_r.close();
        /*unsafe {
            libc::close(0);
        };*/
    }).await;

    stream.shutdown(Shutdown::Both).unwrap();
}
