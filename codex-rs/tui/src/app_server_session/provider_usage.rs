//! Usage reads belong to the server's account provider, independently of thread overrides.

use super::AppServerSession;
use super::ThreadParamsMode;
use crate::legacy_core::config::Config;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;

impl AppServerSession {
    pub(super) async fn load_provider_usage(&mut self) -> Result<()> {
        if self.thread_params_mode() == ThreadParamsMode::Embedded {
            return Ok(());
        }
        let request_id = self.next_request_id();
        let response = self
            .client
            .request_typed::<ModelProviderCapabilitiesReadResponse>(
                ClientRequest::ModelProviderCapabilitiesRead {
                    request_id,
                    params: ModelProviderCapabilitiesReadParams {},
                },
            )
            .await;
        let supports_usage = match response {
            Ok(response) => response.supports_usage,
            Err(TypedRequestError::Server { source, .. })
                if source.code == -32601
                    || source.code == -32600
                        && source.message.contains("modelProvider/capabilities/read")
                        && (source.message.contains("unknown variant")
                            || source.message.contains("unknown method")) =>
            {
                false
            }
            Err(error) => {
                return Err(error).wrap_err(
                    "modelProvider/capabilities/read failed during provider usage bootstrap",
                );
            }
        };
        self.remote_provider_supports_usage = Some(supports_usage);
        Ok(())
    }

    pub(crate) fn sync_provider_usage(&self, config: &mut Config) {
        if let Some(supports_usage) = self.remote_provider_supports_usage {
            config.model_provider.supports_usage = supports_usage;
        }
    }
}
