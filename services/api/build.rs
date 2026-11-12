fn main() {
    // SQLx's stable migrate! macro tracks existing files; track the directory for new migrations too.
    println!("cargo:rerun-if-changed=../../db/migrations");
}
