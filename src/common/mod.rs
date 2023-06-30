use async_std::{prelude::*, channel::{Receiver, RecvError, Sender}};
use serde::{Serialize, Deserialize};
use std::pin::Pin;

#[derive(Serialize, Deserialize, Debug)]
pub struct InitMessage {
    host_extension_name: String,
    args: Vec<String>,
}

impl InitMessage {
    pub fn new(host_extension_name: String, args: Vec<String>) -> Self {
        Self { host_extension_name, args }
    } 

    pub fn host_extension_name(&self) -> &String {
        return &self.host_extension_name;
    }

    pub fn args(&self) -> &Vec<String> {
        return &self.args;
    }
}

pub struct Message {
    fd: u8,
    data: Vec<u8>,
}

impl Message {
    pub fn new(fd: u8, data: Vec<u8>) -> Self {
        Self { fd, data }
    } 

    pub fn fd(&self) -> u8 {
        return self.fd;
    }

    pub fn data(&self) -> &Vec<u8> {
        return &self.data;
    }
}

pub async fn send_msgs_task(receiver: Receiver<Message>, output: &async_std::sync::Mutex<Pin<Box<dyn async_std::io::Write + Send + '_>>>) {
    loop {
        match receiver.recv().await {
            Err(RecvError) => break,
            Ok(msg) => {
                let mut writer = output.lock().await;
                log::debug!("send_msgs_task send: {:?} <- {:?}", msg.fd, msg.data);
                writer.write(&[msg.data.len().try_into().unwrap(), msg.fd]).await;  // FIXME
                writer.write(&msg.data).await;  // FIXME
            }
        }
    }
}

pub async fn recv_msgs_task(sender: &mut Sender<Message>, mut input: Pin<Box<dyn async_std::io::Read + Send>>) {
    let mut buf = vec![0u8; 256];

    loop {
        match input.read_exact(&mut buf[..2]).await {
            Ok(()) => {
                let size = buf[0];
                if size == 0 {
                    log::info!("recv_msgs_task EOF (soft)");
                    break;
                }
                let fd = buf[1];
                let mut data = &mut buf[..size as usize];
                match input.read_exact(&mut data).await {
                    Ok(()) => {
                        log::debug!("recv_msgs_task read: {:?} <- {:?}", fd, data);
                        sender.send(Message::new(fd, Vec::from(data))).await;
                    },
                    Err(err) => {
                        log::error!("recv_msgs_task errored (1): {:?}", err);
                    }
                }
            },
            Err(err) => {
                if err.kind() == async_std::io::ErrorKind::UnexpectedEof {
                    log::info!("recv_msgs_task EOF");
                } else {
                    log::error!("recv_msgs_task errored (2): {:?}", err);
                }
                break;
            }
        }
    }

    sender.close();
    log::info!("recv_msgs_task: sender closed");
}

pub async fn read_io_task(sender: &mut Sender<Message>, fd: u8, mut input: Pin<Box<dyn async_std::io::Read + Send>>) {
    let mut buf = vec![0u8; 32];

    loop {
        match input.read(&mut buf).await {
            Ok(size) => {
                log::debug!("read_io_task read: {:?}", &buf[..size]);
                if size == 0 {
                    log::info!("read_io_task EOF");
                    break;
                }
                sender.send(Message::new(fd, Vec::from(&buf[..size]))).await;
            },
            Err(err) => {
                log::error!("read_io_task errored: {:?}", err);
                break;
            }
        }
    }

    sender.send(Message::new(fd, Vec::new())).await;
    sender.close();
    log::info!("read_io_task: sender closed");
}

pub async fn write_io_task<'a>(receiver: Receiver<Message>, mut stdin: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>, mut stdout: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>, mut stderr: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>) {
    loop {
        match receiver.recv().await {
            Err(RecvError) => break,
            Ok(msg) => {
                let writer_opt = match msg.fd {
                    0 => &mut stdin,
                    1 => &mut stdout,
                    2 => &mut stderr,
                    _ => continue,
                };
                if let Some(ref mut writer) = writer_opt {
                    log::debug!("write_io_task: {:?} <- {:?}", msg.fd, msg.data);
                    if msg.data.len() > 0 {
                        writer.write(&msg.data).await;  // FIXME
                        writer.flush().await;  // FIXME
                    } else {
                        log::info!("write_io_task received EOF (fd {:?})", msg.fd);
                        receiver.close();
                    }
                } else {
                    log::warn!("write_io_task unknown fd (skipping): {:?} <- {:?}", msg.fd, msg.data);
                }
            }
        }
    }
}

pub async fn copy_to_msgs<'a>(mut input_stream: Pin<Box<dyn async_std::io::Read + Send>>, fd: u8, output: &async_std::sync::Mutex<Pin<Box<dyn async_std::io::Write + Send>>>) {
    let mut buf = vec![0u8; 32];
    loop {
        let res = input_stream.read(&mut buf).await;
        match res {
            Ok(size) => {
                log::debug!("copy_to_msgs: read {:?} {:?}", res, &buf[..size]);
                if size == 0 {
                    log::info!("Cmd EOF");
                    break;
                }
                {
                    let mut writer = output.lock().await;
                    writer.write(&[size.try_into().unwrap(), fd]).await;  // FIXME
                    writer.write(&buf[..size]).await;  // FIXME
                }
                buf.clear();
            },
            Err(err) => {
                log::error!("Errored: {:?}", err);
                break;
            }
        }
    }
}

pub async fn copy_from_msgs<'a>(mut input_stream: Pin<Box<dyn async_std::io::Read + Send + 'a>>, mut stdin: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>, mut stdout: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>, mut stderr: Option<&mut Pin<Box<dyn async_std::io::Write + Send + 'a>>>) {
    let mut buf = vec![0u8; 256];
    loop {
        match input_stream.read_exact(&mut buf[..2]).await {
            Ok(()) => {
                let size = buf[0];
                let fd = buf[1];
                log::debug!("copy_from_msgs: read {:?} {:?}", size, fd);
                match input_stream.read(&mut buf[..size.into()]).await {
                    Ok(read) => {
                        let writer_opt = match fd {
                            0 => &mut stdin,
                            1 => &mut stdout,
                            2 => &mut stderr,
                            _ => {
                                continue;
                            }
                        };
                        if let Some(ref mut writer) = writer_opt {
                            writer.write(&buf[..read]).await;  // FIXME
                            log::debug!("copy_from_msgs: {:?} {:?}", buf, read);
                        } else {
                            log::warn!("copy_from_msgs unknown fd (skipping): {:?}", &buf[..read]);
                        }
                    },
                    Err(err) => {
                        log::error!("Errored: {:?}", err);
                        break;
                    }
                }
            },
            Err(err) => {
                if err.kind() == async_std::io::ErrorKind::UnexpectedEof {
                    log::info!("Disconnected");
                } else {
                    log::error!("Errored: {:?}", err);
                }
                break;
            }
        }
    }
}
