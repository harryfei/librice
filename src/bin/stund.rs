// Copyright (C) 2020 Matthew Waters <matthew@centricular.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::net::UdpSocket;

#[macro_use] extern crate log;
use env_logger;

use librice::stun::attribute::*;
use librice::stun::message::*;


fn main() -> std::io::Result<()> {
    env_logger::init();

    let socket = UdpSocket::bind("127.0.0.1:3478")?;

    /* echo server, return errors for all requests */
    loop {
        let mut buf = [0; 1500];
        let (amt, src) = socket.recv_from(&mut buf)?;
        let buf = &buf[..amt];
        trace!("got {:?}", buf);
        let msg = Message::from_bytes(buf).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid message"))?;
        info!("got {:?}", msg);
        if msg.get_type().class() == MessageClass::Request {
            let mtype = MessageType::from_class_method(MessageClass::Error, msg.get_type().method());
            let mut out = Message::new(mtype, msg.transaction_id());
            let attrs = msg.iter_attributes().map(|a| a.get_type()).collect::<Vec<_>>();
            if attrs.len() > 0 {
                 out.add_attribute(UnknownAttributes::new(&attrs).to_raw()).unwrap();
            }
            info!("sending {:?}", out);
            let buf = out.to_bytes();
            trace!("sending {:?}", buf);
            socket.send_to(&buf, &src)?;
        }
    };
}