//! Ownership of the Wayland clipboard selection across both data-control protocols.
use super::wayland_clipboard::MutexBackendState;
use log::{info, warn};
use wayland_client::Proxy;
use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

#[derive(Debug)]
enum DataControl {
    Wlr(ZwlrDataControlManagerV1, ZwlrDataControlDeviceV1),
    Ext(ExtDataControlManagerV1, ExtDataControlDeviceV1),
}

#[derive(Debug)]
enum DataControlSource {
    Wlr(ZwlrDataControlSourceV1),
    Ext(ExtDataControlSourceV1),
}

impl DataControlSource {
    fn id(&self) -> ObjectId {
        match self {
            Self::Wlr(source) => source.id(),
            Self::Ext(source) => source.id(),
        }
    }
}

impl Drop for DataControlSource {
    fn drop(&mut self) {
        match self {
            Self::Wlr(source) => source.destroy(),
            Self::Ext(source) => source.destroy(),
        }
    }
}

#[derive(Debug)]
struct OwnedSelection {
    source: DataControlSource,
    entry_id: u64,
}

#[derive(Debug)]
pub(super) struct SelectionOwner {
    protocol: DataControl,
    qh: QueueHandle<MutexBackendState>,
    connection: Connection,
    active: Option<OwnedSelection>,
}

impl SelectionOwner {
    pub(super) fn bind(
        globals: &GlobalList,
        qh: QueueHandle<MutexBackendState>,
        connection: Connection,
    ) -> Result<Self, &'static str> {
        let seat = globals
            .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=9, ())
            .map_err(
                |_| "The compositor did not expose wl_seat, which is required for clipboard access",
            )?;
        let protocol = if let Ok(manager) =
            globals.bind::<ExtDataControlManagerV1, _, _>(&qh, 1..=1, ())
        {
            info!("Using ext_data_control_manager_v1 clipboard protocol");
            let device = manager.get_data_device(&seat, &qh, ());
            DataControl::Ext(manager, device)
        } else if let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()) {
            info!("Using zwlr_data_control_manager_v1 clipboard protocol");
            let device = manager.get_data_device(&seat, &qh, ());
            DataControl::Wlr(manager, device)
        } else {
            return Err(
                "Clipboard monitoring requires ext_data_control_manager_v1 or zwlr_data_control_manager_v1",
            );
        };
        Ok(Self {
            protocol,
            qh,
            connection,
            active: None,
        })
    }

    pub(super) fn publish(&mut self, entry_id: u64, mimes: impl Iterator<Item = String>) {
        self.clear();
        let source = match &self.protocol {
            DataControl::Ext(manager, device) => {
                let source = manager.create_data_source(&self.qh, ());
                for mime in mimes {
                    source.offer(mime);
                }
                device.set_selection(Some(&source));
                DataControlSource::Ext(source)
            }
            DataControl::Wlr(manager, device) => {
                let source = manager.create_data_source(&self.qh, ());
                for mime in mimes {
                    source.offer(mime);
                }
                device.set_selection(Some(&source));
                DataControlSource::Wlr(source)
            }
        };
        self.active = Some(OwnedSelection { source, entry_id });
        self.flush();
    }

    pub(super) fn entry_id(&self) -> Option<u64> {
        self.active.as_ref().map(|selection| selection.entry_id)
    }

    pub(super) fn entry_for_source(&self, source_id: &ObjectId) -> Option<u64> {
        let active = self.active.as_ref()?;
        (active.source.id() == *source_id).then_some(active.entry_id)
    }

    pub(super) fn cancel(&mut self, source_id: &ObjectId) {
        if self.entry_for_source(source_id).is_some() {
            self.clear();
        }
    }

    pub(super) fn clear(&mut self) {
        self.active = None;
    }

    pub(super) fn revoke(&mut self) {
        match &self.protocol {
            DataControl::Ext(_, device) => device.set_selection(None),
            DataControl::Wlr(_, device) => device.set_selection(None),
        }
        self.clear();
        self.flush();
    }

    fn flush(&self) {
        if let Err(error) = self.connection.flush() {
            warn!("Failed to flush Wayland selection: {error}");
        }
    }
}

impl Drop for SelectionOwner {
    fn drop(&mut self) {
        self.clear();
        match &self.protocol {
            DataControl::Ext(manager, device) => {
                device.destroy();
                manager.destroy();
            }
            DataControl::Wlr(manager, device) => {
                device.destroy();
                manager.destroy();
            }
        }
        self.flush();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use wayland_client::protocol::wl_registry::WlRegistry;

    wayland_client::delegate_noop!(MutexBackendState: ignore WlRegistry);

    // Client-side constructors allocate distinct Wayland object IDs synchronously.
    // Keep the other end open so publishing can flush without a desktop compositor.
    pub(in crate::backend) fn test_owner(use_ext: bool) -> (SelectionOwner, UnixStream) {
        let (socket, peer) = UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue::<MutexBackendState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let seat = registry.bind::<wl_seat::WlSeat, _, _>(1, 1, &qh, ());
        let protocol = if use_ext {
            let manager = registry.bind::<ExtDataControlManagerV1, _, _>(2, 1, &qh, ());
            let device = manager.get_data_device(&seat, &qh, ());
            DataControl::Ext(manager, device)
        } else {
            let manager = registry.bind::<ZwlrDataControlManagerV1, _, _>(2, 2, &qh, ());
            let device = manager.get_data_device(&seat, &qh, ());
            DataControl::Wlr(manager, device)
        };
        (
            SelectionOwner {
                protocol,
                qh,
                connection,
                active: None,
            },
            peer,
        )
    }

    #[test]
    fn only_the_current_source_can_request_or_cancel_its_entry() {
        for use_ext in [true, false] {
            let (mut owner, _peer) = test_owner(use_ext);
            owner.publish(10, std::iter::once("text/plain".to_owned()));
            let old_source = owner.active.as_ref().unwrap().source.id();
            owner.publish(20, std::iter::once("text/plain".to_owned()));
            let current_source = owner.active.as_ref().unwrap().source.id();
            assert_ne!(old_source, current_source);

            assert_eq!(owner.entry_for_source(&old_source), None);
            owner.cancel(&old_source);
            assert_eq!(owner.entry_for_source(&current_source), Some(20));
            owner.cancel(&current_source);
            assert_eq!(owner.entry_for_source(&current_source), None);
            assert_eq!(owner.entry_id(), None);
        }
    }
}
