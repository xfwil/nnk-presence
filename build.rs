fn main() {
    // exe icon shown in Explorer / taskbar
    winresource::WindowsResource::new().set_icon("res/app.ico").compile().expect("embedding icon");
}
