# **gst-cam-rs**

A lightweight, Rust-based webcam viewer and recorder utilizing egui and GStreamer.

## **Features**

* View live webcam feeds with selectable resolution and framerate.  
* Record video to .mp4 (h264 encoding).  
* Capture image snapshots to .png.  
* Real-time video manipulation (flip horizontal/vertical, 90-degree rotations).  
* Configurable output directory.

## **Dependencies**

This application requires GStreamer core and its plugins to handle media pipelines. On Arch Linux / EndeavourOS:

sudo pacman \-S gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly

## **Installation**

### **Cargo**

To build and run directly via Cargo:

cargo run \--release

### **Arch Linux (makepkg)**

To build and install the application globally using the included PKGBUILD:

makepkg \-si

## **Usage**

By default, recordings and images are saved to \~/Videos/gst-cam-rs. You can adjust this path at runtime using the ⚙ Settings menu.
