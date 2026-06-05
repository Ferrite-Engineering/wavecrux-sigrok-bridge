/*
 * bindgen entry point for the `sigrok` feature build.
 *
 * libsigrokdecode.h transitively includes <glib.h>, so allow-listing the
 * handful of GLib symbols we need (GSList traversal, GVariant inspection,
 * GHashTable construction) in build.rs works off this single header.
 */
#include <libsigrokdecode/libsigrokdecode.h>
