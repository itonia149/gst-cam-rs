# Maintainer: Itonia <itonia149@gmail.com>
pkgname=gst-cam-rs
pkgver=0.1.0
pkgrel=1
pkgdesc="A Rust-based webcam capture and recording tool using egui and GStreamer"
arch=('x86_64')
url="https://github.com/yourusername/gst-cam-rs"
license=('MIT')
depends=('gstreamer' 'gst-plugins-base' 'gst-plugins-good' 'gst-plugins-bad' 'gst-plugins-ugly')
makedepends=('cargo')
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --release --locked
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 "target/release/gst-cam-rs" "$pkgdir/usr/bin/$pkgname"
}
