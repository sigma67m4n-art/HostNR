use hostnr_core::hosts::MAX_BYTES;
use jni::{objects::{GlobalRef, JClass, JIntArray, JObject, JString}, sys::jint, JavaVM, JNIEnv};
use std::{collections::VecDeque, fs::File, io::{Read, Write}, os::fd::FromRawFd, sync::{Mutex, OnceLock}};
use winit::platform::android::activity::AndroidApp;

struct Bridge { vm: JavaVM, activity: GlobalRef }
static BRIDGE: OnceLock<Mutex<Bridge>> = OnceLock::new();
static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
static EXPORT: Mutex<Option<String>> = Mutex::new(None);

pub enum Event { File(String, String), Message(String), Text(i32, String) }

pub fn push(event: Event) {
    if let Ok(mut events) = EVENTS.lock() {
        if events.len() < 32 { events.push_back(event); }
    }
}

pub fn events() -> Vec<Event> {
    EVENTS.lock().map(|mut e| e.drain(..).collect()).unwrap_or_default()
}

pub fn init(app: &AndroidApp) -> Result<(), String> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(|e| e.to_string())?;
    let activity = {
        let env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let borrowed = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        env.new_global_ref(borrowed).map_err(|e| e.to_string())?
    };
    let bridge = Bridge { vm, activity };
    if let Some(existing) = BRIDGE.get() { *existing.lock().map_err(|e| e.to_string())? = bridge; }
    else { let _ = BRIDGE.set(Mutex::new(bridge)); }
    Ok(())
}

pub fn action(name: &str) -> Result<(), String> {
    let bridge = BRIDGE.get().ok_or("Android is not ready")?.lock().map_err(|e| e.to_string())?;
    let mut env = bridge.vm.attach_current_thread().map_err(|e| e.to_string())?;
    let result = env.call_method(bridge.activity.as_obj(), name, "()V", &[]);
    if result.is_err() {
        let _ = env.exception_clear();
        return Err("Android action failed".into());
    }
    Ok(())
}

pub fn palette() -> Option<Vec<i32>> {
    let bridge = BRIDGE.get()?.lock().ok()?;
    let mut env = bridge.vm.attach_current_thread().ok()?;
    let result: jni::errors::Result<Vec<i32>> = env.with_local_frame(8, |env| {
        let object = env.call_method(bridge.activity.as_obj(), "palette", "()[I", &[])?.l()?;
        let array = JIntArray::from(object);
        let mut values = vec![0; env.get_array_length(&array)? as usize];
        env.get_int_array_region(&array, 0, &mut values)?;
        Ok(values)
    });
    if result.is_err() { let _ = env.exception_clear(); }
    result.ok()
}

pub fn export(text: String) -> Result<(), String> {
    *EXPORT.lock().map_err(|e| e.to_string())? = Some(text);
    action("exportHosts")
}

pub fn edit(field: i32, text: &str) -> Result<(), String> {
    let bridge = BRIDGE.get().ok_or("Android is not ready")?.lock().map_err(|e| e.to_string())?;
    let mut env = bridge.vm.attach_current_thread().map_err(|e| e.to_string())?;
    let text = env.auto_local(JObject::from(env.new_string(text).map_err(|e| e.to_string())?));
    let result = env.call_method(bridge.activity.as_obj(), "editText", "(ILjava/lang/String;)V", &[
        jni::objects::JValue::Int(field), jni::objects::JValue::Object(text.as_ref()),
    ]);
    if result.is_err() {
        let _ = env.exception_clear();
        return Err("Cannot open keyboard".into());
    }
    Ok(())
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostActivity_nativeText(mut env: JNIEnv, _class: JClass, field: jint, text: JString) {
    let text: Result<String, _> = env.get_string(&text).map(|s| s.into());
    if let Ok(text) = text {
        if let Ok(mut events) = EVENTS.lock() {
            events.retain(|e| !matches!(e, Event::Text(id, _) if *id == field));
            events.push_back(Event::Text(field, text.chars().take(8192).collect()));
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostActivity_nativeFile(mut env: JNIEnv, _class: JClass, fd: jint, name: JString) {
    if fd < 0 { return; }
    let name = env.get_string(&name).map(|s| s.into()).unwrap_or_else(|_| "Imported hosts".into());
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = unsafe { File::from_raw_fd(fd) }.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes);
        if let Err(e) = result { push(Event::Message(e.to_string())); return; }
        if bytes.len() > MAX_BYTES { push(Event::Message("File exceeds 8 MiB".into())); return; }
        match String::from_utf8(bytes) {
            Ok(text) => push(Event::File(name, text)),
            Err(_) => push(Event::Message("File must be UTF-8 text".into())),
        }
    });
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostActivity_nativeExport(_env: JNIEnv, _class: JClass, fd: jint) {
    if fd < 0 { return; }
    let text = EXPORT.lock().ok().and_then(|mut s| s.take());
    std::thread::spawn(move || {
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = text.ok_or_else(|| "No hosts to export".to_string())
            .and_then(|text| file.write_all(text.as_bytes()).map_err(|e| e.to_string()));
        push(Event::Message(match result { Ok(()) => "Hosts exported successfully".into(), Err(e) => e }));
    });
}

#[no_mangle]
pub extern "system" fn Java_app_hostnr_HostActivity_nativeMessage(mut env: JNIEnv, _class: JClass, message: JString) {
    let message: Result<String, _> = env.get_string(&message).map(|s| s.into());
    if let Ok(message) = message { push(Event::Message(message)); }
}
