use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::LogFormat;

pub fn init_tracing(log_format: LogFormat) {
    let registry = tracing_subscriber::registry().with(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
    );

    match log_format {
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(false)
                    .with_span_list(false),
            )
            .init(),
        LogFormat::Human => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    struct BufferWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = BufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            BufferWriter(self.0.clone())
        }
    }

    impl io::Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn json_logging_emits_structured_fields() {
        let sink = Buffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_writer(sink.clone()),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "astropay::test", invoice_state = "paid", "json log works");
        });

        let output = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("\"level\":\"INFO\""));
        assert!(output.contains("\"target\":\"astropay::test\""));
        assert!(output.contains("\"message\":\"json log works\""));
        assert!(output.contains("\"invoice_state\":\"paid\""));
    }
}
