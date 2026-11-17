use std::sync::Arc;

use agent_runtime_protocol::domain::ports::Transport;
use agent_runtime_protocol::domain::schema::v0::{ToRuntimeMessage, ToServerMessage};
use agent_session::domain::model::{AgentSessionId, SandboxSize};
use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};
use dashmap::DashMap;
use tracing::Instrument as _;

use super::client::NamespaceClient;
use super::errors::NamespaceError;
use super::types::{ContainerSpec, Instance, NamespaceSettings};
use crate::domain::error::{HarnessError, Result};
use crate::domain::model::SpawnContainer;
use crate::domain::ports::ContainerManager;
use crate::domain::sandbox::{SandboxResizeEffect, create_only_resize_effect, resources};
use crate::outbound::provision;
use crate::outbound::sidecar::SidecarTransport;

#[cfg(test)]
mod test;

/// Hands out Namespace instances.
pub struct NamespaceContainerManager {
    client: NamespaceClient,
    image_ref: super::types::ImageRef,
    lifetime: std::time::Duration,
    /// The instance of each session this process spawned, which is the
    /// only lookup from a session to its instance there is yet: a command
    /// reaches the replica that holds the session, which spawned it.
    instances: DashMap<AgentSessionId, Arc<Instance>>,
}

impl NamespaceContainerManager {
    /// Build the manager from its settings.
    #[must_use]
    pub fn new(settings: NamespaceSettings) -> Self {
        let NamespaceSettings {
            api_url,
            token,
            image_ref,
            lifetime,
        } = settings;
        Self {
            client: NamespaceClient::new(api_url, token),
            image_ref,
            lifetime,
            instances: DashMap::new(),
        }
    }

    #[tracing::instrument(
        name = "agent.container.boot",
        err,
        skip(self),
        fields(agent.container.provider = "namespace", agent.container.id = %instance.id)
    )]
    async fn bring_up(&self, instance: Instance) -> Result<NamespaceContainer> {
        self.client
            .wait_until_ready(&instance.id, provision::ENSURE_TIMEOUT)
            .await
            .map_err(unavailable)?;
        let output = self
            .client
            .run_command(
                &instance,
                &["bash", "-lc", &provision::ensure_ready_command()],
            )
            .instrument(tracing::info_span!("agent.container.ensure_ready"))
            .await
            .map_err(unavailable)?;
        // The readiness recipe reads an answer without a code as a pass, as
        // it always has.
        let exit_code = output.exit_code.unwrap_or(0);
        if exit_code != 0 {
            return Err(unavailable(NamespaceError::ReadinessRecipe {
                instance_id: instance.id.to_string(),
                exit_code,
                stdout: output.stdout,
                stderr: output.stderr,
            }));
        }

        let sidecar_url = self
            .client
            .create_ingress(&instance.id, provision::SIDECAR_PORT)
            .await
            .map_err(unavailable)?;
        let socket = dial_sidecar(&sidecar_url)
            .instrument(tracing::info_span!("agent.container.websocket_connect"))
            .await
            .map_err(unavailable)?;

        Ok(NamespaceContainer {
            instance: Arc::new(instance),
            client: self.client.clone(),
            wire: SidecarTransport::connect(socket),
        })
    }
}

impl ContainerManager for NamespaceContainerManager {
    type Transport = NamespaceContainer;

    // `skip_all`: `SpawnContainer` carries the egress session token; nothing
    // secret-bearing may be Debug-recorded into the span.
    #[tracing::instrument(
        name = "agent.container.spawn",
        err,
        skip_all,
        fields(
            agent.container.provider = "namespace",
            session_id = %command.session_id,
        )
    )]
    async fn spawn(
        &self,
        command: SpawnContainer,
    ) -> Result<agent_session::domain::connection::RuntimeAttachment<Self::Transport>> {
        let mut env = Vec::new();
        env.extend(command.egress.environment());
        let container = ContainerSpec {
            image_ref: self.image_ref.clone(),
            env,
            exported_ports: vec![provision::SIDECAR_PORT],
        };
        let instance = self
            .client
            .create_instance(&container, self.lifetime, resources(command.size))
            .await
            .map_err(unavailable)?;
        tracing::info!(instance_id = %instance.id, url = %instance.url, "instance created");
        let instance_id = instance.id.clone();

        match self.bring_up(instance).await {
            Ok(container) => {
                self.instances
                    .insert(command.session_id, Arc::clone(&container.instance));
                Ok(agent_session::domain::connection::RuntimeAttachment::solo(
                    container,
                ))
            }
            Err(error) => {
                if let Err(destroy_error) = self.client.destroy_instance(&instance_id).await {
                    tracing::error!(
                        error = ?destroy_error,
                        instance_id = %instance_id,
                        "failed to destroy an instance that never came up"
                    );
                }
                Err(error)
            }
        }
    }

    fn resize_effect(&self, from: SandboxSize, to: SandboxSize) -> SandboxResizeEffect {
        create_only_resize_effect(from, to)
    }

    async fn resize(&self, _session: AgentSessionId, _size: SandboxSize) -> Result<()> {
        Err(HarnessError::Container(
            "Namespace instances cannot be resized in place".to_owned(),
        ))
    }

    async fn resume(
        &self,
        _session: AgentSessionId,
    ) -> Result<agent_session::domain::connection::RuntimeAttachment<Self::Transport>> {
        todo!("resuming Namespace instances is not implemented yet")
    }

    async fn session_token(&self, _session: AgentSessionId) -> Result<Option<String>> {
        // Instances do hold a token in their environment, but reading it back
        // needs the instance id for a session - the same lookup `resume` is
        // waiting on.
        todo!("recovering a Namespace instance's session token is not implemented yet")
    }

    /// Runs in the instance this process spawned for the session; one it
    /// did not spawn is not known here.
    #[tracing::instrument(err, skip(self, command))]
    async fn exec(
        &self,
        session: AgentSessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandOutput> {
        let instance = self
            .instances
            .get(&session)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| {
                HarnessError::Container(format!(
                    "session {session} has no instance spawned here to run a command in"
                ))
            })?;
        let script = command.script(provision::CHECKOUT);
        let argv = ["bash", "-c", script.as_str()];
        let run = self.client.run_command(&instance, &argv);
        let output = tokio::time::timeout(command.timeout() + provision::EXEC_MARGIN, run)
            .await
            .map_err(|_| {
                HarnessError::Container(format!(
                    "instance {} did not answer the command in time",
                    instance.id
                ))
            })?
            .map_err(unavailable)?;
        // A command's code is its verdict: one Namespace did not give is
        // not a pass.
        let exit_code = output.exit_code.ok_or_else(|| {
            HarnessError::Container(format!(
                "instance {} gave no exit code for the command",
                instance.id
            ))
        })?;
        Ok(SessionCommandOutput::new(
            exit_code,
            output.stdout.as_bytes(),
            output.stderr.as_bytes(),
        ))
    }

    /// Destroys the instance this process spawned for the session. One it
    /// did not spawn needs the lookup `resume` is waiting on.
    async fn teardown(&self, session: AgentSessionId) -> Result<()> {
        let Some((_, instance)) = self.instances.remove(&session) else {
            return Err(HarnessError::Container(format!(
                "session {session} has no instance spawned here to tear down"
            )));
        };
        self.client
            .destroy_instance(&instance.id)
            .await
            .map_err(unavailable)
    }
}

/// One Namespace instance and its live sidecar transport.
pub struct NamespaceContainer {
    instance: Arc<Instance>,
    client: NamespaceClient,
    wire: SidecarTransport,
}

impl NamespaceContainer {
    /// Destroy the instance, logging a provider failure rather than masking the run result.
    pub async fn release(&self) {
        let _ = self
            .client
            .destroy_instance(&self.instance.id)
            .await
            .inspect_err(|error| {
                tracing::error!(
                    error = ?error,
                    instance_id = %self.instance.id,
                    "instance destroy failed"
                );
            });
    }
}

impl Transport<ToRuntimeMessage, ToServerMessage> for NamespaceContainer {
    type Sender = crate::outbound::sidecar::SidecarSender;
    type Receiver = tokio::sync::mpsc::UnboundedReceiver<ToServerMessage>;

    /// The sandbox itself is not carried into the halves: nothing reattaches
    /// to a container object once its session has it, and ending a sandbox
    /// goes through the manager by session id.
    fn split(self) -> (Self::Sender, Self::Receiver) {
        self.wire.split()
    }
}

async fn dial_sidecar(
    ingress_url: &str,
) -> std::result::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    NamespaceError,
> {
    let mut ws_url =
        url::Url::parse(ingress_url).map_err(|source| NamespaceError::InvalidIngressUrl {
            url: ingress_url.to_owned(),
            source,
        })?;
    let websocket_scheme = match ws_url.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => {
            return Err(NamespaceError::UnsupportedIngressScheme {
                scheme: scheme.to_owned(),
            });
        }
    };
    ws_url
        .set_scheme(websocket_scheme)
        .map_err(|()| NamespaceError::UnsupportedIngressScheme {
            scheme: websocket_scheme.to_owned(),
        })?;
    let (socket, _) = tokio_tungstenite::connect_async(ws_url.as_str())
        .await
        .map_err(NamespaceError::WebSocketConnect)?;
    Ok(socket)
}

fn unavailable(error: NamespaceError) -> HarnessError {
    HarnessError::Container(error.to_string())
}
