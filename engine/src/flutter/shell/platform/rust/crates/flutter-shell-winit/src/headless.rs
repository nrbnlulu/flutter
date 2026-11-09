//! Display-free adapter for the existing merged-thread runtime and plugin SDK.
#[path = "headless/agent.rs"]
mod agent;
use super::*;
use flutter_shell_wgpu::{OffscreenBroker, OffscreenFrame};
use std::cell::Cell;
use std::sync::mpsc::{Receiver, SyncSender};

struct VirtualView {
    _broker: Box<OffscreenBroker>,
    kind: NativeWindowKind,
    parent: Option<FlutterRustViewId>,
    title: String,
    bounds: (f64, f64, f64, f64),
    constraints: Option<(f64, f64, f64, f64)>,
    shrink_wrap: bool,
    state: FlutterRustWindowState,
    removing: bool,
    ready: bool,
    placement: Option<PopupPlacement>,
    restore_bounds: Option<(f64, f64, f64, f64)>,
}
struct Registry {
    views: BTreeMap<FlutterRustViewId, VirtualView>,
    next_id: i64,
    callback: Option<FlutterRustWindowEventCallback>,
}
struct VirtualHost {
    registry: RefCell<Registry>,
    context: Arc<GpuContext>,
    events: HostEventSender,
    frames: SyncSender<Result<OffscreenFrame, String>>,
    ratio: f64,
    capture: bool,
    shell: Cell<*mut c_void>,
    state_path: Option<PathBuf>,
    agent: RefCell<Option<Arc<agent::Shared>>>,
    input: RefCell<agent::InputState>,
}
impl VirtualHost {
    fn apply_text_commands(&self, inbox: &TextInputInbox) {
        for command in inbox.drain() {
            let mut input = self.input.borrow_mut();
            if matches!(&command, TextInputCommand::SetClient(..)) {
                input.text_view = self
                    .registry
                    .borrow()
                    .views
                    .iter()
                    .find(|(_, v)| v.state.focused != 0 && !v.removing)
                    .map(|(id, _)| *id);
            } else if matches!(&command, TextInputCommand::ClearClient) {
                input.text_view = None;
            }
            input.text.apply(command);
        }
    }
    fn metrics(&self, width: f64, height: f64) -> WindowMetrics {
        let physical = |value: f64| (value * self.ratio).round().max(1.) as u32;
        let (width, height) = (physical(width), physical(height));
        WindowMetrics {
            width,
            height,
            min_width: width,
            max_width: width,
            min_height: height,
            max_height: height,
            pixel_ratio: self.ratio,
            display_width: 1920,
            display_height: 1080,
            display_refresh_rate: 60.,
        }
    }
    fn view_metrics(&self, view: &VirtualView) -> WindowMetrics {
        let mut metrics = self.metrics(view.bounds.2, view.bounds.3);
        if view.shrink_wrap {
            let (a, b, c, d) = view.constraints.unwrap_or((1., 1., 1920., 1080.));
            metrics.min_width = (a * self.ratio).round().max(1.) as u32;
            metrics.min_height = (b * self.ratio).round().max(1.) as u32;
            metrics.max_width = (c * self.ratio).round().min(f64::from(
                self.context.device().limits().max_texture_dimension_2d,
            )) as u32;
            metrics.max_height = (d * self.ratio).round().min(f64::from(
                self.context.device().limits().max_texture_dimension_2d,
            )) as u32;
        }
        metrics
    }
    fn refresh_dimensions(&self) {
        let changes = {
            let mut r = self.registry.borrow_mut();
            let origins = r
                .views
                .iter()
                .map(|(id, v)| (*id, (v.bounds.0, v.bounds.1)))
                .collect::<BTreeMap<_, _>>();
            r.views
                .iter_mut()
                .filter_map(|(id, v)| {
                    if !v.shrink_wrap || v.removing {
                        return None;
                    }
                    let (w, h) = v._broker.dimensions()?;
                    let (w, h) = (f64::from(w) / self.ratio, f64::from(h) / self.ratio);
                    if v.bounds.2 == w && v.bounds.3 == h {
                        return None;
                    }
                    v.bounds.2 = w;
                    v.bounds.3 = h;
                    v.state.width = w;
                    v.state.height = h;
                    if let Some(mut placement) = v.placement {
                        placement.width = w.round() as u32;
                        placement.height = h.round() as u32;
                        let origin = v
                            .parent
                            .and_then(|p| origins.get(&p))
                            .copied()
                            .unwrap_or((0., 0.));
                        v.bounds = platform::positioner::place(
                            placement,
                            origin,
                            (-1920., -1080., 3840., 2160.),
                        );
                        v.placement = Some(placement);
                    }
                    Some((*id, self.view_metrics(v)))
                })
                .collect::<Vec<_>>()
        };
        for (id, metrics) in changes {
            set_cpp_shell_viewport_metrics(self.shell.get(), id, metrics);
            self.notify(id, FlutterRustWindowEvent::StateChanged);
        }
    }
    fn snapshot(&self) -> Value {
        let r = self.registry.borrow();
        serde_json::json!({"views":r.views.iter().map(|(id,v)|serde_json::json!({
            "viewId":id.0,"parentViewId":v.parent.map(|id|id.0),"kind":format!("{:?}",v.kind).to_lowercase(),
            "title":v.title,"bounds":[v.bounds.0,v.bounds.1,v.bounds.2,v.bounds.3],"pixelRatio":self.ratio,
            "focused":v.state.focused!=0,"ready":v.ready,"removing":v.removing,
            "visible":v.state.minimized==0,"sizeGeneration":v._broker.generation(),
        })).collect::<Vec<_>>()})
    }
    fn notify(&self, id: FlutterRustViewId, event: FlutterRustWindowEvent) {
        if let Some(agent) = self.agent.borrow().as_ref() {
            agent.publish_windows(self.snapshot());
        }
        if let Some(path) = &self.state_path {
            if let Err(error) = std::fs::write(path, serde_json::to_vec(&self.snapshot()).unwrap())
            {
                log::error!("headless state artifact: {error}");
            }
        }
        let callback = self.registry.borrow().callback;
        if let Some(callback) = callback {
            callback(id, event);
        }
    }
    fn valid_size(&self, width: f64, height: f64) -> bool {
        valid_window_size(width, height)
            && width * self.ratio
                <= f64::from(self.context.device().limits().max_texture_dimension_2d)
            && height * self.ratio
                <= f64::from(self.context.device().limits().max_texture_dimension_2d)
    }
    fn resize(&self, id: FlutterRustViewId, width: f64, height: f64) {
        if !self.valid_size(width, height) {
            return;
        }
        {
            let mut registry = self.registry.borrow_mut();
            let origin = registry
                .views
                .get(&id)
                .and_then(|v| v.parent)
                .and_then(|p| registry.views.get(&p))
                .map(|v| (v.bounds.0, v.bounds.1))
                .unwrap_or((0., 0.));
            let Some(view) = registry.views.get_mut(&id).filter(|v| !v.removing) else {
                return;
            };
            let (mut w, mut h) = (width, height);
            if let Some((min_w, min_h, max_w, max_h)) = view.constraints {
                w = w.clamp(min_w, max_w);
                h = h.clamp(min_h, max_h);
            }
            if !self.valid_size(w, h) {
                return;
            }
            view.bounds.2 = w;
            view.bounds.3 = h;
            view.state.width = w;
            view.state.height = h;
            if let Some(mut placement) = view.placement {
                placement.width = w.round() as u32;
                placement.height = h.round() as u32;
                view.bounds =
                    platform::positioner::place(placement, origin, (-1920., -1080., 3840., 2160.));
                view.placement = Some(placement);
            }
        }
        let metrics = self
            .registry
            .borrow()
            .views
            .get(&id)
            .map(|v| self.view_metrics(v));
        if let Some(metrics) = metrics {
            set_cpp_shell_viewport_metrics(self.shell.get(), id, metrics);
        }
        self.notify(id, FlutterRustWindowEvent::StateChanged);
    }
    fn focus(&self, id: FlutterRustViewId) {
        let changed = {
            let mut registry = self.registry.borrow_mut();
            if !registry
                .views
                .get(&id)
                .is_some_and(|v| !v.removing && v.kind != NativeWindowKind::Tooltip)
            {
                return;
            }
            let mut id = id;
            for _ in 0..registry.views.len() {
                let mut chain = HashSet::new();
                let mut ancestor = Some(id);
                while let Some(current) = ancestor {
                    chain.insert(current);
                    ancestor = registry.views.get(&current).and_then(|v| v.parent);
                }
                let modal = registry
                    .views
                    .iter()
                    .filter(|(other, v)| {
                        !chain.contains(other)
                            && v.parent.is_some_and(|p| chain.contains(&p))
                            && v.kind == NativeWindowKind::Dialog
                            && !v.removing
                            && v.state.minimized == 0
                    })
                    .map(|(id, _)| *id)
                    .max();
                match modal {
                    Some(modal) => id = modal,
                    None => break,
                }
            }
            let mut changed = Vec::new();
            for (view_id, view) in &mut registry.views {
                let focused = i32::from(*view_id == id);
                if view.state.focused != focused {
                    view.state.focused = focused;
                    changed.push((*view_id, focused));
                }
            }
            changed
        };
        for (view_id, focused) in changed {
            if focused == 0 {
                agent::release_for_view(self, view_id);
                let popup = self
                    .registry
                    .borrow()
                    .views
                    .get(&view_id)
                    .is_some_and(|v| v.kind == NativeWindowKind::Popup && !v.removing);
                if popup {
                    self.notify(view_id, FlutterRustWindowEvent::CloseRequested);
                }
            }
            send_cpp_view_focus_event(
                self.shell.get(),
                view_id,
                if focused != 0 {
                    FlutterRustViewFocusState::Focused
                } else {
                    FlutterRustViewFocusState::Unfocused
                },
            );
            self.notify(view_id, FlutterRustWindowEvent::StateChanged);
        }
    }
    fn position(&self, id: FlutterRustViewId, request: NativePopupRequest) -> bool {
        let mut registry = self.registry.borrow_mut();
        let Some(parent) = registry
            .views
            .get(&request.parent_view_id)
            .filter(|v| !v.removing)
        else {
            return false;
        };
        let origin = (parent.bounds.0, parent.bounds.1);
        let Some(view) = registry.views.get_mut(&id).filter(|v| {
            !v.removing && v.parent == Some(request.parent_view_id) && v.kind == request.kind
        }) else {
            return false;
        };
        let placement = PopupPlacement {
            width: view.bounds.2.round() as u32,
            height: view.bounds.3.round() as u32,
            anchor_rect: request.anchor_rect,
            anchor: request.parent_anchor,
            gravity: request.gravity,
            offset: request.offset,
            constraint_adjustment: request.constraint_adjustment,
        };
        view.bounds =
            platform::positioner::place(placement, origin, (-1920., -1080., 3840., 2160.));
        view.placement = Some(placement);
        drop(registry);
        self.notify(id, FlutterRustWindowEvent::StateChanged);
        true
    }
    fn completed(&self, id: FlutterRustViewId, operation: ViewOperation, succeeded: bool) {
        match operation {
            ViewOperation::Add if succeeded => {
                let focus = {
                    let mut r = self.registry.borrow_mut();
                    let Some(view) = r.views.get_mut(&id) else {
                        return;
                    };
                    view.ready = true;
                    view.kind != NativeWindowKind::Tooltip && !view.removing
                };
                if focus {
                    self.focus(id);
                }
                self.notify(id, FlutterRustWindowEvent::StateChanged);
            }
            ViewOperation::Add | ViewOperation::Remove => {
                let removed = self.registry.borrow_mut().views.remove(&id);
                if let Some(view) = removed {
                    let was_focused = view.state.focused != 0;
                    let parent = view.parent;
                    drop(view); // C++ has completed raster detach before this event.
                    self.notify(id, FlutterRustWindowEvent::Destroyed);
                    if was_focused {
                        if let Some(parent) = parent {
                            self.focus(parent);
                        } else {
                            self.focus(FlutterRustViewId::IMPLICIT);
                        }
                    }
                }
            }
        }
    }
}
impl WindowingCallbackHost for VirtualHost {
    fn create_window(
        &self,
        request: NativeWindowRequest,
        kind: NativeWindowKind,
        parent: Option<FlutterRustViewId>,
    ) -> FlutterRustViewId {
        if !self.valid_size(request.width, request.height) {
            return FlutterRustViewId(-1);
        }
        let (id, callbacks, metrics) = {
            let mut registry = self.registry.borrow_mut();
            if parent.is_some_and(|p| !registry.views.get(&p).is_some_and(|v| !v.removing)) {
                return FlutterRustViewId(-1);
            }
            let id = FlutterRustViewId(registry.next_id);
            registry.next_id += 1;
            let (mut width, mut height) = (request.width, request.height);
            if let Some((a, b, c, d)) = request.constraints {
                width = width.clamp(a, c);
                height = height.clamp(b, d);
            }
            if !self.valid_size(width, height) {
                return FlutterRustViewId(-1);
            }
            let origin = parent
                .and_then(|p| registry.views.get(&p))
                .map(|v| (v.bounds.0 + 24., v.bounds.1 + 24.))
                .unwrap_or((0., 0.));
            let broker = OffscreenBroker::new(self.context.clone(), self.frames.clone());
            broker.set_capture_enabled(self.capture);
            broker.set_view_id(id.0);
            let events = self.events.clone();
            broker.set_notification(move || {
                let _ = events.send_event(HostEvent::TaskScheduled);
            });
            let callbacks = unsafe { broker.callbacks() };
            registry.views.insert(
                id,
                VirtualView {
                    _broker: broker,
                    kind,
                    parent,
                    title: request.title,
                    bounds: (origin.0, origin.1, width, height),
                    constraints: request.constraints,
                    shrink_wrap: request.shrink_wrap,
                    state: FlutterRustWindowState {
                        width,
                        height,
                        focused: 0,
                        maximized: 0,
                        minimized: 0,
                        fullscreen: 0,
                    },
                    removing: false,
                    ready: false,
                    placement: None,
                    restore_bounds: None,
                },
            );
            let metrics = self.view_metrics(registry.views.get(&id).unwrap());
            (id, callbacks, metrics)
        };
        if let Some(agent) = self.agent.borrow().as_ref() {
            agent.publish_windows(self.snapshot());
        }
        add_cpp_shell_view(
            self.shell.get(),
            id,
            metrics,
            callbacks,
            self.events.clone(),
        );
        id
    }
    fn create_popup(&self, request: NativePopupRequest) -> FlutterRustViewId {
        let (min_w, min_h, max_w, max_h) = request.constraints;
        let size = (min_w.max(1.).min(max_w), min_h.max(1.).min(max_h));
        let parent = request.parent_view_id;
        let kind = request.kind;
        let id = self.create_window(
            NativeWindowRequest {
                title: String::new(),
                width: size.0,
                height: size.1,
                shrink_wrap: true,
                resizable: false,
                constraints: Some(request.constraints),
            },
            kind,
            Some(parent),
        );
        if id.0 > 0 {
            self.position(id, request);
        }
        id
    }
    fn create_satellite(&self, request: NativeSatelliteRequest) -> FlutterRustViewId {
        let parent = request.parent_view_id;
        let id = self.create_window(request.window, NativeWindowKind::Satellite, Some(parent));
        if let Some(rect) = request.anchor_rect {
            let r = NativePopupRequest {
                kind: NativeWindowKind::Satellite,
                parent_view_id: parent,
                constraints: (1., 1., 1920., 1080.),
                anchor_rect: (
                    rect.0.round() as i32,
                    rect.1.round() as i32,
                    rect.2.round() as i32,
                    rect.3.round() as i32,
                ),
                parent_anchor: request.parent_anchor,
                gravity: request.gravity,
                offset: (
                    request.offset.0.round() as i32,
                    request.offset.1.round() as i32,
                ),
                constraint_adjustment: request._constraint_adjustment,
            };
            if id.0 > 0 {
                self.position(id, r);
            }
        }
        id
    }
    fn destroy_window(&self, id: FlutterRustViewId) {
        let removals = {
            let mut r = self.registry.borrow_mut();
            if id == FlutterRustViewId::IMPLICIT {
                return;
            }
            let mut ids = vec![id];
            let mut index = 0;
            while index < ids.len() {
                let parent = ids[index];
                let children: Vec<_> = r
                    .views
                    .iter()
                    .filter_map(|(id, v)| {
                        (v.parent == Some(parent) && !ids.contains(id)).then_some(*id)
                    })
                    .collect();
                ids.extend(children);
                index += 1;
            }
            ids.into_iter()
                .rev()
                .filter(|id| {
                    if let Some(v) = r.views.get_mut(id)
                        && !v.removing
                    {
                        v.removing = true;
                        true
                    } else {
                        false
                    }
                })
                .collect::<Vec<_>>()
        };
        for id in removals {
            agent::release_for_view(self, id);
            remove_cpp_shell_view(self.shell.get(), id, self.events.clone());
        }
    }
}

// C++ invokes these synchronous callbacks only on the owning merged thread.
unsafe fn host<'a>(data: *mut c_void) -> &'a VirtualHost {
    unsafe { &*data.cast::<VirtualHost>() }
}
extern "C" fn regular(
    d: *mut c_void,
    r: *const FlutterRustRegularWindowRequest,
) -> FlutterRustViewId {
    dispatch_create_regular(unsafe { host(d) }, r)
}
extern "C" fn dialog(
    d: *mut c_void,
    r: *const FlutterRustDialogWindowRequest,
) -> FlutterRustViewId {
    dispatch_create_dialog(unsafe { host(d) }, r)
}
extern "C" fn popup(d: *mut c_void, r: *const FlutterRustPopupWindowRequest) -> FlutterRustViewId {
    dispatch_create_popup(unsafe { host(d) }, r)
}
extern "C" fn satellite(
    d: *mut c_void,
    r: *const FlutterRustSatelliteWindowRequest,
) -> FlutterRustViewId {
    dispatch_create_satellite(unsafe { host(d) }, r)
}
extern "C" fn destroy(d: *mut c_void, id: FlutterRustViewId) {
    dispatch_destroy(unsafe { host(d) }, id);
}
extern "C" fn get_state(
    d: *mut c_void,
    id: FlutterRustViewId,
    out: *mut FlutterRustWindowState,
) -> i32 {
    if out.is_null() {
        return 0;
    }
    let h = unsafe { host(d) };
    let r = h.registry.borrow();
    let Some(view) = r.views.get(&id).filter(|v| !v.removing) else {
        return 0;
    };
    unsafe {
        out.write(view.state);
    };
    1
}
extern "C" fn size(d: *mut c_void, id: FlutterRustViewId, w: f64, h: f64) {
    unsafe { host(d) }.resize(id, w, h);
}
extern "C" fn constraints(
    d: *mut c_void,
    id: FlutterRustViewId,
    has: i32,
    a: f64,
    b: f64,
    c: f64,
    e: f64,
) {
    if has != 0 && !valid_constraints(a, b, c, e) {
        return;
    }
    let host = unsafe { host(d) };
    let size = {
        let mut r = host.registry.borrow_mut();
        let Some(v) = r.views.get_mut(&id) else {
            return;
        };
        v.constraints = (has != 0).then_some((a, b, c, e));
        (v.bounds.2, v.bounds.3)
    };
    host.resize(id, size.0, size.1);
}
extern "C" fn title(d: *mut c_void, id: FlutterRustViewId, value: *const u8, length: u64) {
    let Ok(length) = usize::try_from(length) else {
        return;
    };
    if length != 0 && value.is_null() {
        return;
    }
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value, length) }
    };
    if let Ok(title) = std::str::from_utf8(bytes)
        && let Some(v) = unsafe { host(d) }.registry.borrow_mut().views.get_mut(&id)
    {
        v.title = title.to_owned();
    }
}
extern "C" fn activate(d: *mut c_void, id: FlutterRustViewId) {
    unsafe { host(d) }.focus(id);
}
extern "C" fn maximized(d: *mut c_void, id: FlutterRustViewId, enabled: i32) {
    flag(d, id, enabled, 0);
}
extern "C" fn minimized(d: *mut c_void, id: FlutterRustViewId, enabled: i32) {
    flag(d, id, enabled, 1);
}
extern "C" fn fullscreen(d: *mut c_void, id: FlutterRustViewId, enabled: i32) {
    flag(d, id, enabled, 2);
}
fn flag(d: *mut c_void, id: FlutterRustViewId, enabled: i32, which: u8) {
    let h = unsafe { host(d) };
    let metrics = {
        let mut r = h.registry.borrow_mut();
        let Some(v) = r.views.get_mut(&id).filter(|v| !v.removing) else {
            return;
        };
        let enabled = i32::from(enabled != 0);
        match which {
            0 => v.state.maximized = enabled,
            1 => v.state.minimized = enabled,
            _ => v.state.fullscreen = enabled,
        };
        if which != 1 {
            if v.state.maximized != 0 || v.state.fullscreen != 0 {
                if v.restore_bounds.is_none() {
                    v.restore_bounds = Some(v.bounds);
                }
                v.bounds = (0., 0., 1920. / h.ratio, 1080. / h.ratio);
            } else if let Some(bounds) = v.restore_bounds.take() {
                v.bounds = bounds;
            }
            v.state.width = v.bounds.2;
            v.state.height = v.bounds.3;
        }
        h.view_metrics(v)
    };
    if which != 1 {
        set_cpp_shell_viewport_metrics(h.shell.get(), id, metrics);
    }
    h.notify(id, FlutterRustWindowEvent::StateChanged);
}

extern "C" fn event_callback(d: *mut c_void, callback: Option<FlutterRustWindowEventCallback>) {
    unsafe { host(d) }.registry.borrow_mut().callback = callback;
}
extern "C" fn parent(d: *mut c_void, id: FlutterRustViewId, parent: FlutterRustViewId) -> i32 {
    let h = unsafe { host(d) };
    let mut r = h.registry.borrow_mut();
    if id == parent || !r.views.get(&parent).is_some_and(|v| !v.removing) {
        return 0;
    }
    let mut ancestor = Some(parent);
    while let Some(a) = ancestor {
        if a == id {
            return 0;
        }
        ancestor = r.views.get(&a).and_then(|v| v.parent);
    }
    let Some(v) = r
        .views
        .get_mut(&id)
        .filter(|v| v.kind == NativeWindowKind::Satellite && !v.removing)
    else {
        return 0;
    };
    v.parent = Some(parent);
    1
}
extern "C" fn reposition(
    d: *mut c_void,
    id: FlutterRustViewId,
    r: *const FlutterRustPopupWindowRequest,
) -> i32 {
    if r.is_null() {
        return 0;
    }
    decode_popup_request(unsafe { &*r })
        .is_some_and(|r| unsafe { host(d) }.position(id, r))
        .into()
}
fn callbacks(h: &VirtualHost) -> FlutterRustWindowingCallbacks {
    FlutterRustWindowingCallbacks {
        user_data: (h as *const VirtualHost).cast_mut().cast(),
        create_regular_window: Some(regular),
        create_dialog_window: Some(dialog),
        create_popup_window: Some(popup),
        create_satellite_window: Some(satellite),
        destroy_window: Some(destroy),
        get_window_state: Some(get_state),
        set_window_size: Some(size),
        set_window_constraints: Some(constraints),
        set_window_title: Some(title),
        activate_window: Some(activate),
        set_window_maximized: Some(maximized),
        set_window_minimized: Some(minimized),
        set_window_fullscreen: Some(fullscreen),
        set_window_event_callback: Some(event_callback),
        set_window_parent: Some(parent),
        update_popup_position: Some(reposition),
    }
}

fn engine_context(
    raw: flutter_shell_wgpu::VulkanContextData,
    callback: impl FnOnce(FlutterRustVulkanContextData) -> *mut c_void,
) -> *mut c_void {
    let ie: Vec<_> = raw
        .instance_extensions
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap())
        .collect();
    let de: Vec<_> = raw
        .device_extensions
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap())
        .collect();
    let ip: Vec<_> = ie.iter().map(|s| s.as_ptr()).collect();
    let dp: Vec<_> = de.iter().map(|s| s.as_ptr()).collect();
    callback(FlutterRustVulkanContextData {
        get_instance_proc_addr: raw.get_instance_proc_addr as *mut c_void,
        instance: raw.instance as *mut c_void,
        physical_device: raw.physical_device as *mut c_void,
        device: raw.device as *mut c_void,
        queue: raw.queue as *mut c_void,
        queue_family_index: raw.queue_family_index,
        instance_extensions: ip.as_ptr(),
        instance_extensions_count: ip.len() as u32,
        device_extensions: dp.as_ptr(),
        device_extensions_count: dp.len() as u32,
    })
}

pub(super) fn run(
    config: ShellConfig,
    registration: ApplicationRegistration,
) -> Result<(), RunError> {
    run_inner(config, registration).map_err(RunError::Headless)
}
fn run_inner(config: ShellConfig, registration: ApplicationRegistration) -> Result<(), String> {
    // Explicitly select headless before creating any native event loop, window,
    // clipboard or display handle. Display environment variables may be present.
    let width = std::env::var("FLUTTER_RUST_HEADLESS_WIDTH")
        .unwrap_or_else(|_| "1280".into())
        .parse::<f64>()
        .map_err(|_| "invalid headless width")?;
    let height = std::env::var("FLUTTER_RUST_HEADLESS_HEIGHT")
        .unwrap_or_else(|_| "800".into())
        .parse::<f64>()
        .map_err(|_| "invalid headless height")?;
    let ratio = std::env::var("FLUTTER_RUST_HEADLESS_PIXEL_RATIO")
        .unwrap_or_else(|_| "1".into())
        .parse::<f64>()
        .map_err(|_| "invalid pixel ratio")?;
    if !ratio.is_finite() || ratio <= 0. {
        return Err("pixel ratio must be finite and positive".into());
    }
    let events = HostEventSender::headless();
    let dispatcher_events = events.clone();
    let dispatcher = MainThreadDispatcher::for_shell_inactive(
        move |task| {
            dispatcher_events
                .send_event(HostEvent::MainThreadTask(task))
                .is_ok()
        },
        std::thread::current().id(),
    );
    let mut registrar = PluginRegistrar::for_shell(dispatcher.clone());
    let retained = Arc::new(Mutex::new(Vec::new()));
    let tasks = Box::new(TaskRunnerHost::with_wake_proxy(Some(events.clone())));
    tasks.install_cpp_task_runner();
    let vsync = Box::new(VsyncHost::new(events.clone()));
    let mut inbox = Box::new(TextInputInbox::new(events.clone()));
    inbox.headless = true;
    let context = GpuContext::headless()?;
    log::info!("Headless Vulkan adapter: {:?}", context.adapter_info());
    let (frames, completed) = std::sync::mpsc::sync_channel(4);
    let artifacts = std::env::var_os("FLUTTER_RUST_HEADLESS_ARTIFACTS").map(PathBuf::from);
    if let Some(path) = &artifacts {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    let host = Box::new(VirtualHost {
        registry: RefCell::new(Registry {
            views: BTreeMap::new(),
            next_id: 1,
            callback: None,
        }),
        context: context.clone(),
        events: events.clone(),
        frames,
        ratio,
        capture: artifacts.is_some(),
        shell: Cell::new(std::ptr::null_mut()),
        state_path: std::env::var_os("FLUTTER_RUST_HEADLESS_STATE").map(PathBuf::from),
        agent: RefCell::new(None),
        input: RefCell::new(agent::InputState::new()),
    });
    if !host.valid_size(width, height) {
        return Err("invalid or unsupported headless dimensions".into());
    }
    let broker = OffscreenBroker::new(context.clone(), host.frames.clone());
    broker.set_capture_enabled(host.capture);
    let notify = events.clone();
    broker.set_notification(move || {
        let _ = notify.send_event(HostEvent::TaskScheduled);
    });
    let assets = CString::new(config.assets_path).map_err(|e| e.to_string())?;
    let icu = CString::new(config.icu_data_path).map_err(|e| e.to_string())?;
    let aot = CString::new(config.aot_library_path).map_err(|e| e.to_string())?;
    let settings = FlutterRustShellSettings {
        assets_path: assets.as_ptr(),
        icu_data_path: icu.as_ptr(),
        aot_library_path: aot.as_ptr(),
    };
    let shell = context
        .with_vulkan_context(|raw| {
            engine_context(raw, |raw| {
                create_cpp_shell(
                    tasks.task_runner_handle(),
                    raw,
                    unsafe { broker.callbacks() },
                    inbox.callbacks(),
                    vsync.callbacks(true),
                    callbacks(&host),
                    settings,
                )
            })
        })
        .ok_or("Vulkan context extraction failed")?;
    if shell.is_null() {
        return Err("engine creation failed".into());
    }
    host.shell.set(shell);
    host.registry.borrow_mut().views.insert(
        FlutterRustViewId::IMPLICIT,
        VirtualView {
            _broker: broker,
            kind: NativeWindowKind::Regular,
            parent: None,
            title: config.title,
            bounds: (0., 0., width, height),
            constraints: None,
            shrink_wrap: false,
            state: FlutterRustWindowState {
                width,
                height,
                focused: 1,
                maximized: 0,
                minimized: 0,
                fullscreen: 0,
            },
            removing: false,
            ready: true,
            placement: None,
            restore_bounds: None,
        },
    );
    unsafe {
        context.install_queue_lock(cpp_queue_lock_hooks(shell));
    }
    let mut agent_session = None;
    let result = (|| {
        if run_cpp_shell(shell) == 0 {
            return Err("Flutter isolate failed to start".into());
        }
        set_cpp_shell_viewport_metrics(
            shell,
            FlutterRustViewId::IMPLICIT,
            host.metrics(width, height),
        );
        registrar.install_gpu_for_shell(GpuTextures::for_shell(Arc::new(
            ShellWgpuTextureBackend {
                context: context.clone(),
                dispatcher: dispatcher.clone(),
                shell_address: shell as usize,
                retained: retained.clone(),
            },
        )));
        dispatcher.start_for_shell();
        let mut registration = Some(registration);
        register_application_once(&mut registration, &mut registrar)
            .map_err(|e| format!("plugin registration: {e:?}"))?;
        send_cpp_lifecycle_event(shell, FlutterRustLifecycleState::Resumed);
        send_cpp_view_focus_event(
            shell,
            FlutterRustViewId::IMPLICIT,
            FlutterRustViewFocusState::Focused,
        );
        if let Some(path) = std::env::var_os("FLUTTER_RUST_AGENT_SOCKET") {
            let artifacts = std::env::var_os("FLUTTER_RUST_AGENT_ARTIFACTS")
                .map(PathBuf::from)
                .ok_or("agent artifacts directory is required")?;
            let session = agent::Session::start(&host, PathBuf::from(path), artifacts)?;
            *host.agent.borrow_mut() = Some(session.shared.clone());
            agent_session = Some(session);
        }
        host.notify(
            FlutterRustViewId::IMPLICIT,
            FlutterRustWindowEvent::StateChanged,
        );
        event_loop(
            &host,
            &events,
            &tasks,
            &vsync,
            &inbox,
            &completed,
            artifacts.as_deref(),
        )
    })();
    if let Some(session) = agent_session.as_mut() {
        session.stop();
    }
    agent::release_inputs(&host);
    dispatcher.shutdown_for_shell();
    // Unregister while callback owners, queue hooks and the raster runner live.
    for registration in retained.lock().iter() {
        registration.unregistered.store(true, Ordering::Release);
        let reclamation = Box::new(TextureReclamation {
            texture_id: registration.texture_id,
            retained: retained.clone(),
        });
        unregister_cpp_external_texture(
            shell,
            registration.texture_id,
            Some(reclaim_external_texture),
            Box::into_raw(reclamation).cast(),
        );
    }
    send_cpp_lifecycle_event(shell, FlutterRustLifecycleState::Detached);
    destroy_cpp_shell(shell); // drains raster, releases every view's target
    context.clear_queue_lock();
    retained.lock().clear();
    host.registry.borrow_mut().views.clear();
    host.shell.set(std::ptr::null_mut());
    result
}

fn event_loop(
    host: &VirtualHost,
    events: &HostEventSender,
    tasks: &TaskRunnerHost,
    vsync: &VsyncHost,
    inbox: &TextInputInbox,
    completed: &Receiver<Result<OffscreenFrame, String>>,
    artifacts: Option<&std::path::Path>,
) -> Result<(), String> {
    let interval = Duration::from_nanos(16_666_667);
    let mut next_pulse = None;
    let mut last_pulse = Instant::now() - interval;
    loop {
        if let Some(agent) = host.agent.borrow().as_ref() {
            if let Some(error) = agent.failure() {
                return Err(error.to_string());
            }
        }
        for event in events.drain() {
            match event {
                HostEvent::TaskScheduled => {
                    host.refresh_dimensions();
                    host.apply_text_commands(inbox);
                }
                HostEvent::VsyncRequested => {
                    if next_pulse.is_none() {
                        next_pulse = Some((last_pulse + interval).max(Instant::now()));
                    }
                }
                HostEvent::MainThreadTask(task) => task(),
                HostEvent::ViewOperationCompleted {
                    view_id,
                    operation,
                    succeeded,
                } => host.completed(view_id, operation, succeeded),
                HostEvent::RequestAppExit { response } => {
                    send_cancelable_exit_request(host.shell.get(), inbox.exit.clone(), response)
                }
                HostEvent::ExitRequested => return Ok(()),
            }
        }
        tasks.dispatch_due_tasks();
        host.apply_text_commands(inbox);
        for command in inbox.drain_cursor_commands() {
            if let Some(response) = command.response {
                complete_cpp_platform_message(response, STANDARD_METHOD_SUCCESS);
            }
        }
        for title in inbox.drain_window_titles() {
            if let Some(view) = host
                .registry
                .borrow_mut()
                .views
                .get_mut(&FlutterRustViewId::IMPLICIT)
            {
                view.title = title;
            }
        }
        if next_pulse.is_some_and(|t| t <= Instant::now()) {
            next_pulse = None;
            if vsync.take_request() {
                last_pulse = Instant::now();
                send_cpp_vsync(host.shell.get(), interval.as_nanos() as u64);
            }
        }
        while let Ok(frame) = completed.try_recv() {
            let frame = frame?;
            if let Some(agent) = host.agent.borrow().as_ref() {
                agent.publish_frame(frame);
                continue;
            }
            if let Some(path) = artifacts {
                std::fs::write(
                    path.join(format!(
                        "view-{}-generation-{}-frame-{}-{}x{}.rgba",
                        frame.view_id, frame.generation, frame.frame, frame.width, frame.height
                    )),
                    frame.rgba,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        if let Some(agent) = host.agent.borrow().as_ref() {
            agent.publish_windows(host.snapshot());
            for message in std::mem::take(&mut *inbox.agent_messages.lock()) {
                agent.publish_bridge(message);
            }
        }
        let deadline = match (tasks.next_deadline(), next_pulse) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        events.wait_until(deadline);
    }
}
