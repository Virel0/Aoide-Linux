//! Single instance through the bus: whoever holds `io.github.Virel0.Aoide` is
//! the running app, and a later launch calls its `Raise` method and exits.
//! The bus daemon grants a name to one connection at a time, so two launches
//! racing each other still end with exactly one first.

use std::time::Duration;

use tokio::sync::mpsc::{self, Sender};
use zbus::fdo::{self, RequestNameFlags};
use zbus::{Connection, interface};

use super::builder;
use crate::{APP_BUS_NAME, APP_INTERFACE, APP_OBJECT_PATH, Bus, Instance, Result};

/// How long a second launch waits for the first to answer `Raise`.
const RAISE_TIMEOUT: Duration = Duration::from_secs(5);

/// Holds the app's bus name. Dropping it gives the name up.
#[derive(Debug)]
pub struct InstanceGuard {
    _connection: Connection,
}

struct App {
    raise: Sender<()>,
}

#[interface(name = "io.github.Virel0.Aoide")]
impl App {
    /// Asks the running app to show its window.
    fn raise(&self) -> fdo::Result<()> {
        // One pending raise is as good as many.
        let _ = self.raise.try_send(());
        Ok(())
    }
}

pub async fn single_instance(bus: &Bus) -> Result<Instance> {
    let (tx, raise) = mpsc::channel(1);
    // Serve the object before asking for the name, so a raise that arrives
    // the moment the name is ours is not lost.
    let connection = builder(bus)?
        .serve_at(APP_OBJECT_PATH, App { raise: tx })?
        .build()
        .await?;
    match connection
        .request_name_with_flags(APP_BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(_) => Ok(Instance::First {
            guard: InstanceGuard {
                _connection: connection,
            },
            raise,
        }),
        Err(zbus::Error::NameTaken) => {
            let call = connection.call_method(
                Some(APP_BUS_NAME),
                APP_OBJECT_PATH,
                Some(APP_INTERFACE),
                "Raise",
                &(),
            );
            let raised = matches!(tokio::time::timeout(RAISE_TIMEOUT, call).await, Ok(Ok(_)));
            Ok(Instance::Other { raised })
        }
        Err(e) => Err(e.into()),
    }
}
