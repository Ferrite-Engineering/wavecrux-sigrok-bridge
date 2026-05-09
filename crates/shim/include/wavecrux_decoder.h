// wavecrux_decoder.h — vendored copy of the WaveCrux open-core decoder
// plugin C ABI header.
//
// Source: github.com/wavecrux/wavecrux include/wavecrux_decoder.h
//
// This file is vendored so the shim crate can build without depending on
// the WaveCrux open-core checkout. When the upstream header changes in a
// way that bumps the ABI MAJOR version, mirror the change here and bump
// the shim's reported version.
//
// License: this header is part of WaveCrux open-core, which transitions
// to Apache-2.0 post-beta. Pre-beta it ships under WaveCrux's pre-release
// license. Either is compatible with this repo's GPLv3+.

#ifndef WAVECRUX_DECODER_H
#define WAVECRUX_DECODER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define WAVECRUX_DECODER_ABI_MAJOR 1
#define WAVECRUX_DECODER_ABI_MINOR 0

#define WAVECRUX_DECODER_ABI_VERSION \
    ((((uint32_t)WAVECRUX_DECODER_ABI_MAJOR) << 16) | \
     ((uint32_t)WAVECRUX_DECODER_ABI_MINOR))

#define WAVECRUX_DECODER_ABI_GET_MAJOR(v) (((uint32_t)(v) >> 16) & 0xFFFFu)
#define WAVECRUX_DECODER_ABI_GET_MINOR(v) ((uint32_t)(v) & 0xFFFFu)

#define WC_DECODER_OK              0
#define WC_DECODER_ERR             1
#define WC_DECODER_NEED_MORE_SLOTS 2

typedef void* WcDecoderHandle;

typedef struct WcSample {
    uint64_t       timestamp_fs;
    const uint8_t* bits_ptr;
    uint32_t       bit_width;
    uint32_t       _reserved0;
} WcSample;

typedef struct WcTransaction {
    uint64_t    start_fs;
    uint64_t    end_fs;
    const char* label;
    const char* fields_json;
    uint32_t    is_error;
    uint32_t    _reserved0;
} WcTransaction;

typedef WcDecoderHandle (*WcDecoderCreateFn)(const char* config_json);
typedef int32_t (*WcDecoderFeedFn)(
    WcDecoderHandle handle,
    const WcSample* sample,
    WcTransaction*  out_transactions,
    size_t*         inout_count);
typedef int32_t (*WcDecoderFlushFn)(
    WcDecoderHandle handle,
    WcTransaction*  out_transactions,
    size_t*         inout_count);
typedef void (*WcDecoderDestroyFn)(WcDecoderHandle handle);

typedef struct WcDecoderDef {
    const char* id;
    const char* display_name;
    const char* manifest_json;
    WcDecoderCreateFn  create;
    WcDecoderFeedFn    feed;
    WcDecoderFlushFn   flush;
    WcDecoderDestroyFn destroy;
    void*    _reserved0;
    uint64_t _reserved1;
} WcDecoderDef;

uint32_t wavecrux_decoder_abi_version(void);
int32_t  wavecrux_decoder_register(WcDecoderDef* out_defs,
                                   size_t*       inout_count);

#ifdef __cplusplus
}
#endif

#endif
