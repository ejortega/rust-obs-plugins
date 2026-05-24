use std::rc::Rc;
use xcb::{x, XidNew};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
pub struct WindowSnapshot {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,

    #[allow(dead_code)]
    pub root_width: f32,
    #[allow(dead_code)]
    pub root_height: f32,
}

pub struct ActiveWindow {
    connection: Rc<xcb::Connection>,
    active_window_atom: x::Atom,
    pub window: x::Window,
    root: x::Window,
    root_width: f32,
    root_height: f32,
}

impl ActiveWindow {
    fn new(
        connection: Rc<xcb::Connection>,
        active_window_atom: x::Atom,
        root: x::Window,
        root_width: f32,
        root_height: f32,
    ) -> Result<ActiveWindow> {
        let window = Self::get_active_window(&connection, active_window_atom, root).unwrap_or(root);

        let active_window = Self {
            window,
            connection,
            active_window_atom,
            root,
            root_width,
            root_height,
        };

        active_window.start_listening()?;

        Ok(active_window)
    }

    /// Read `_NET_ACTIVE_WINDOW` off the root window. This is the single EWMH
    /// property the filter needs, so we query it directly rather than pulling in
    /// an EWMH helper crate.
    fn get_active_window(
        connection: &xcb::Connection,
        active_window_atom: x::Atom,
        root: x::Window,
    ) -> Result<x::Window> {
        let cookie = connection.send_request(&x::GetProperty {
            delete: false,
            window: root,
            property: active_window_atom,
            r#type: x::ATOM_WINDOW,
            long_offset: 0,
            long_length: 1,
        });
        let reply = connection.wait_for_reply(cookie)?;
        let id = *reply.value::<u32>().first().ok_or("no active window set")?;
        Ok(x::Window::new(id))
    }

    fn set_event_mask(&self, mask: x::EventMask) -> Result<()> {
        self.connection
            .send_and_check_request(&x::ChangeWindowAttributes {
                window: self.window,
                value_list: &[x::Cw::EventMask(mask)],
            })?;
        Ok(())
    }

    fn stop_listening(&self) -> Result<()> {
        if self.window == self.root {
            Ok(())
        } else {
            self.set_event_mask(x::EventMask::empty())
        }
    }

    fn start_listening(&self) -> Result<()> {
        self.set_event_mask(
            x::EventMask::PROPERTY_CHANGE
                | x::EventMask::FOCUS_CHANGE
                | x::EventMask::STRUCTURE_NOTIFY
                | x::EventMask::SUBSTRUCTURE_NOTIFY
                | x::EventMask::LEAVE_WINDOW,
        )
    }

    fn update(&mut self) -> Result<()> {
        let window = Self::get_active_window(&self.connection, self.active_window_atom, self.root)
            .unwrap_or(self.root);

        if self.window != window {
            self.stop_listening().unwrap_or(());
            self.window = window;
            self.start_listening()?;
        }

        Ok(())
    }

    fn snapshot(&self) -> Result<WindowSnapshot> {
        let geom = {
            let cookie = self.connection.send_request(&x::GetGeometry {
                drawable: x::Drawable::Window(self.window),
            });
            self.connection.wait_for_reply(cookie)?
        };

        let diff = {
            let cookie = self.connection.send_request(&x::TranslateCoordinates {
                src_window: self.window,
                dst_window: geom.root(),
                src_x: geom.x(),
                src_y: geom.y(),
            });
            self.connection.wait_for_reply(cookie)?
        };

        let snap = WindowSnapshot {
            x: diff.dst_x() as f32,
            y: diff.dst_y() as f32,
            width: geom.width() as f32,
            height: geom.height() as f32,
            root_width: self.root_width,
            root_height: self.root_height,
        };

        Ok(snap)
    }
}

impl Drop for ActiveWindow {
    fn drop(&mut self) {
        self.stop_listening().unwrap_or(());
    }
}

pub struct Server {
    connection: Rc<xcb::Connection>,
    active: ActiveWindow,
}

impl Server {
    pub fn new() -> Result<Server> {
        let (connection, default_screen) = xcb::Connection::connect(None)?;
        let connection = Rc::new(connection);

        let active_window_atom = {
            let cookie = connection.send_request(&x::InternAtom {
                only_if_exists: true,
                name: b"_NET_ACTIVE_WINDOW",
            });
            connection.wait_for_reply(cookie)?.atom()
        };

        let (root, root_width, root_height) = {
            let screen = connection
                .get_setup()
                .roots()
                .nth(default_screen as usize)
                .ok_or("no screen for default screen number")?;
            (
                screen.root(),
                screen.width_in_pixels() as f32,
                screen.height_in_pixels() as f32,
            )
        };

        connection.send_and_check_request(&x::ChangeWindowAttributes {
            window: root,
            value_list: &[x::Cw::EventMask(x::EventMask::SUBSTRUCTURE_NOTIFY)],
        })?;

        Ok(Server {
            active: ActiveWindow::new(
                Rc::clone(&connection),
                active_window_atom,
                root,
                root_width,
                root_height,
            )?,
            connection,
        })
    }

    pub fn wait_for_event(&mut self) -> Option<WindowSnapshot> {
        if self.connection.wait_for_event().is_ok() {
            if self.active.update().is_err() {
                None
            } else {
                self.active.snapshot().ok()
            }
        } else {
            None
        }
    }
}
