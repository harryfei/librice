// Copyright (C) 2020 Matthew Waters <matthew@centricular.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::sync::{Arc, Mutex};

use async_std::net::{SocketAddr, UdpSocket};

use futures::prelude::*;
use futures::Stream;

use crate::agent::AgentError;
use crate::candidate::Candidate;

use crate::socket::{SocketChannel, UdpConnectionChannel, UdpSocketChannel};

use crate::stun::agent::StunAgent;
use crate::stun::message::MessageIntegrityCredentials;

pub const RTP: usize = 1;
pub const RTCP: usize = 2;

#[derive(Debug)]
pub struct Component {
    pub id: usize,
    inner: Arc<Mutex<ComponentInner>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ComponentState {
    New,
    Gathering,
    Connected,
    Failed,
}

impl Component {
    pub(crate) fn new(id: usize) -> Self {
        Self {
            id,
            inner: Arc::new(Mutex::new(ComponentInner::new(id))),
        }
    }

    /// Retreive the current state of a `Component`
    ///
    /// # Examples
    ///
    /// The initial state is `ComponentState::New`
    ///
    /// ```
    /// # use librice::component::{Component, ComponentState};
    /// # use librice::agent::Agent;
    /// # use librice::stream::Stream;
    /// let agent = Agent::default();
    /// let stream = agent.add_stream();
    /// let component = stream.add_component().unwrap();
    /// assert_eq!(component.state(), ComponentState::New);
    /// ```
    pub fn state(&self) -> ComponentState {
        let inner = self.inner.lock().unwrap();
        inner.state
    }

    // XXX: temporary for bring up
    pub async fn set_local_addr(&self, addr: SocketAddr) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        inner.set_local_addr(addr).await
    }

    // XXX: temporary for bring up
    pub async fn set_local_channel(
        &self,
        channel: Arc<UdpSocketChannel>,
    ) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        inner.set_local_channel(channel).await
    }

    // XXX: temporary for bring up
    pub async fn set_remote_addr(&self, addr: SocketAddr) -> Result<(), AgentError> {
        let mut inner = self.inner.lock().unwrap();
        inner.set_remote_addr(addr).await
    }

    // XXX: temporary for bring up
    pub fn local_addr(&self) -> Option<SocketAddr> {
        let inner = self.inner.lock().unwrap();
        inner.socket.clone().and_then(move |s| s.local_addr().ok())
    }

    // XXX: temporary for bring up
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        let inner = self.inner.lock().unwrap();
        inner
            .channel
            .clone()
            .and_then(move |s| s.remote_addr().ok())
    }

    /// Retrieve a Stream that produces data sent to this component from a peer
    pub fn receive_stream(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        let inner = self.inner.lock().unwrap();
        // TODO: this probably may need to be multiplexed from multiple sources on e.g. candidate
        // changes
        let c = inner.channel.clone();
        c.and_then(move |c| Some(c.receive_stream()))
    }

    /// Send data to the peer using the established communication channel
    pub async fn send(&self, data: &[u8]) -> Result<(), AgentError> {
        let channel = {
            let inner = self.inner.lock().unwrap();
            inner.channel.clone().ok_or(AgentError::ResourceNotFound)?
        };
        channel.send(data).await?;
        Ok(())
    }

    pub(crate) async fn gather_stream(
        &self,
        local_credentials: MessageIntegrityCredentials,
        remote_credentials: MessageIntegrityCredentials,
    ) -> Result<impl Stream<Item = (Candidate, Arc<StunAgent>)>, AgentError> {
        let stun_servers = {
            let inner = self.inner.lock().unwrap();
            inner.stun_servers.clone()
        };
        let schannels = crate::gathering::iface_udp_sockets()?
            .filter_map(move |s| async move { s.ok() })
            .collect::<Vec<_>>()
            .await;

        let agents = {
            let mut inner = self.inner.lock().unwrap();
            for channel in schannels.iter() {
                let agent = Arc::new(StunAgent::new(channel.clone()));
                agent.set_local_credentials(local_credentials.clone());
                agent.set_remote_credentials(remote_credentials.clone());
                inner.agents.push(agent);
            }
            inner.agents.clone()
        };

        info!("retreived sockets");
        Ok(
            crate::gathering::gather_component(1, schannels, stun_servers)?.map(
                move |(cand, channel)| {
                    (
                        cand,
                        agents
                            .iter()
                            .find(|agent| {
                                agent.channel.local_addr().ok() == channel.local_addr().ok()
                            })
                            .unwrap()
                            .clone(),
                    )
                },
            ),
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ComponentInner {
    pub(crate) id: usize,
    state: ComponentState,
    //selected_pair: Option<CandidatePair>,
    socket: Option<Arc<UdpSocketChannel>>,
    channel: Option<Arc<SocketChannel>>,
    pub(crate) stun_agent: Option<Arc<StunAgent>>,
    stun_servers: Vec<SocketAddr>,
    turn_servers: Vec<SocketAddr>,
    agents: Vec<Arc<StunAgent>>,
}

impl ComponentInner {
    fn new(id: usize) -> Self {
        Self {
            id,
            state: ComponentState::New,
            //selected_pair: None,
            socket: None,
            channel: None,
            stun_agent: None,
            stun_servers: vec![],
            turn_servers: vec![],
            agents: vec![],
        }
    }

    // XXX: temporary for bring-up
    pub async fn set_local_addr(&mut self, addr: SocketAddr) -> Result<(), AgentError> {
        let udp = UdpSocket::bind(addr).await?;
        self.set_local_channel(Arc::new(UdpSocketChannel::new(udp)))
            .await
    }

    // XXX: temporary for bring-up
    pub async fn set_local_channel(
        &mut self,
        channel: Arc<UdpSocketChannel>,
    ) -> Result<(), AgentError> {
        self.socket = Some(channel);
        self.state = ComponentState::Gathering;
        Ok(())
    }

    // XXX: temporary for bring-up
    pub async fn set_remote_addr(&mut self, addr: SocketAddr) -> Result<(), AgentError> {
        let socket = self.socket.clone().ok_or(AgentError::ResourceNotFound)?;
        self.channel = Some(Arc::new(SocketChannel::Udp(UdpConnectionChannel::new(
            socket.clone(),
            addr,
        ))));
        self.stun_agent = Some(Arc::new(StunAgent::new(socket)));
        self.state = ComponentState::Connected;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::stun::message::ShortTermCredentials;

    fn init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn initial_state_new() {
        init();
        let a = Agent::default();
        let s = a.add_stream();
        let c = s.add_component().unwrap();
        assert_eq!(c.state(), ComponentState::New);
    }

    #[test]
    fn set_addrs() {
        init();
        async_std::task::block_on(async move {
            let a = Agent::default();
            let s = a.add_stream();
            let c = s.add_component().unwrap();
            assert_eq!(c.state(), ComponentState::New);
            c.set_local_addr("127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            assert_eq!(c.state(), ComponentState::Gathering);
            c.set_remote_addr("127.0.0.1:9000".parse().unwrap())
                .await
                .unwrap();
            assert_eq!(c.state(), ComponentState::Connected);
        });
    }

    #[test]
    fn send_recv() {
        init();
        async_std::task::block_on(async move {
            let a = Agent::default();
            let s = a.add_stream();
            let send = s.add_component().unwrap();
            // XXX: not technically valid usage but works for now
            let recv = s.add_component().unwrap();
            send.set_local_addr("127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            recv.set_local_addr("127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            send.set_remote_addr(recv.local_addr().unwrap())
                .await
                .unwrap();
            recv.set_remote_addr(send.local_addr().unwrap())
                .await
                .unwrap();
            let data = vec![3; 4];
            let recv_stream = recv.receive_stream().unwrap();
            futures::pin_mut!(recv_stream);
            send.send(&data).await.unwrap();
            let res = recv_stream.next().await.unwrap();
            assert_eq!(data, res);
        });
    }

    #[test]
    fn gather() {
        init();
        async_std::task::block_on(async move {
            // attempt to gather some candidates
            // assumes we have non-localhost networking available
            let send_credentials = MessageIntegrityCredentials::ShortTerm(ShortTermCredentials {
                password: "send".into(),
            });
            let recv_credentials = MessageIntegrityCredentials::ShortTerm(ShortTermCredentials {
                password: "recv".into(),
            });
            let a = Agent::default();
            let s = a.add_stream();
            let send = s.add_component().unwrap();
            // assumes the first candidate works
            let send_stream = send
                .gather_stream(send_credentials.clone(), recv_credentials.clone())
                .await
                .unwrap();
            futures::pin_mut!(send_stream);
            let send_cand = send_stream.next().await.unwrap();

            // XXX: not technically valid usage but works for now
            let recv = s.add_component().unwrap();
            let recv_stream = recv
                .gather_stream(recv_credentials, send_credentials)
                .await
                .unwrap();
            futures::pin_mut!(recv_stream);
            // assumes the first candidate works
            let recv_cand = recv_stream.next().await.unwrap();

            send.set_local_channel(send_cand.1.channel.clone())
                .await
                .unwrap();
            recv.set_local_channel(recv_cand.1.channel.clone())
                .await
                .unwrap();
            send.set_remote_addr(recv_cand.0.address).await.unwrap();
            recv.set_remote_addr(send_cand.0.address).await.unwrap();
            assert_eq!(send.state(), ComponentState::Connected);
            assert_eq!(recv.state(), ComponentState::Connected);

            // two-way connection has been setup
            let data = vec![3; 4];
            let recv_recv_stream = recv.receive_stream().unwrap();
            futures::pin_mut!(recv_recv_stream);
            send.send(&data).await.unwrap();
            let res = recv_recv_stream.next().await.unwrap();
            assert_eq!(data, res);

            let data = vec![2; 4];
            let send_recv_stream = send.receive_stream().unwrap();
            futures::pin_mut!(send_recv_stream);
            recv.send(&data).await.unwrap();
            let res = send_recv_stream.next().await.unwrap();
            assert_eq!(data, res);
        });
    }
}
