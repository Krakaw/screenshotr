fn main() {
    // The screencapturekit Swift bridge links libswift_Concurrency.dylib via
    // @rpath while every other Swift runtime library resolves by absolute path,
    // and it adds no LC_RPATH of its own. Without this the binary dies at
    // launch with "Library not loaded: @rpath/libswift_Concurrency.dylib".
    // The OS ships the library in the dyld shared cache under /usr/lib/swift.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
