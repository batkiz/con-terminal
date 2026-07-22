//! Bundle IoskeleyMono into the DirectWrite font collection.
//!
//! On Windows we can't assume the user has IoskeleyMono installed
//! system-wide. We embed the TTFs at compile time (same blobs GPUI
//! registers for UI text — `crates/con-app/src/theme.rs`) and build a
//! custom `IDWriteFontCollection` via `IDWriteFactory5`'s in-memory
//! loader. The glyph atlas consumes this collection for all
//! `CreateTextFormat` calls so we get the designed terminal font
//! regardless of install state.
//!
//! The bundled TTFs are Nerd-Font-patched (ahatem/IoskeleyMono
//! release 2026.03.19-7), adding ~10,400 PUA glyphs — Powerline
//! separators, devicons, Font Awesome, Octicons, Codicons, Material
//! Design. The patched files advertise family name "IoskeleyMono" (we
//! rewrote the `name` table from "IoskeleyMono Nerd Font" at bundle
//! time) so everything that already asks for "IoskeleyMono" resolves
//! unchanged but now renders prompt-theme glyphs natively.
//!
//! Returns `None` if the host runtime lacks `IDWriteFactory5` (pre-
//! Windows 10 1607). The caller falls back to the system collection,
//! which on unbundled machines resolves "IoskeleyMono" to a default
//! system font (Segoe / Consolas). We log a warning in that case.

use anyhow::{Context, Result};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_UNICODE_RANGE, IDWriteFactory, IDWriteFactory2, IDWriteFactory5, IDWriteFont1,
    IDWriteFontCollection, IDWriteFontFallback, IDWriteFontFile, IDWriteFontSet,
    IDWriteFontSetBuilder1, IDWriteInMemoryFontFileLoader,
};
use windows::core::{Interface, PCWSTR};

/// Family name the bundled TTFs advertise (must match the `name` table's
/// family-name record inside the TTF). `IoskeleyMono-*.ttf` follow the
/// Iosevka convention: concatenated family name with no space.
pub const BUNDLED_FONT_FAMILY: &str = "IoskeleyMono";

const FONT_REGULAR: &[u8] = include_bytes!("../../../../../assets/fonts/IoskeleyMono-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../../../assets/fonts/IoskeleyMono-Bold.ttf");
const FONT_ITALIC: &[u8] = include_bytes!("../../../../../assets/fonts/IoskeleyMono-Italic.ttf");
const FONT_BOLD_ITALIC: &[u8] =
    include_bytes!("../../../../../assets/fonts/IoskeleyMono-BoldItalic.ttf");

/// Build a private `IDWriteFontCollection` containing the bundled
/// IoskeleyMono weights. Returns `Ok(None)` when the runtime doesn't
/// support `IDWriteFactory5` (loader API added in Windows 10 1607).
pub fn build_bundled_collection(dwrite: &IDWriteFactory) -> Result<Option<IDWriteFontCollection>> {
    // Cast up: IDWriteFactory → IDWriteFactory5. The shared factory
    // returned by DWriteCreateFactory on Windows 10+ implements this
    // interface; on older hosts the cast fails and we fall back.
    let factory5: IDWriteFactory5 = match dwrite.cast() {
        Ok(f) => f,
        Err(err) => {
            log::warn!(
                "IoskeleyMono bundling skipped: IDWriteFactory5 not \
                 available ({err:?}); falling back to system font \
                 collection"
            );
            return Ok(None);
        }
    };

    // SAFETY: factory5 owned above; the returned loader is retained by
    // us (and by the factory via RegisterFontFileLoader) for the life
    // of the process.
    let loader: IDWriteInMemoryFontFileLoader = unsafe { factory5.CreateInMemoryFontFileLoader() }
        .context("CreateInMemoryFontFileLoader failed")?;
    // SAFETY: loader COM-refcount is bumped by Register; safe to hand
    // the same reference.
    unsafe { factory5.RegisterFontFileLoader(&loader) }.context("RegisterFontFileLoader failed")?;

    // SAFETY: factory5 owns the font-set builder. `CreateFontSetBuilder`
    // on `IDWriteFactory5` returns the `...1` flavour in the Win10+ SDK
    // we pin; it inherits from `IDWriteFontSetBuilder`, but windows-rs
    // binds the concrete type so we accept it here.
    let builder: IDWriteFontSetBuilder1 =
        unsafe { factory5.CreateFontSetBuilder() }.context("CreateFontSetBuilder failed")?;

    for (label, bytes) in [
        ("regular", FONT_REGULAR),
        ("bold", FONT_BOLD),
        ("italic", FONT_ITALIC),
        ("bold_italic", FONT_BOLD_ITALIC),
    ] {
        // SAFETY: bytes are `&'static` (from `include_bytes!`), so the
        // pointer stays valid for the process lifetime. `None` owner
        // is fine when the caller guarantees the data outlives the
        // font file reference.
        let file: IDWriteFontFile = unsafe {
            loader.CreateInMemoryFontFileReference(
                dwrite,
                bytes.as_ptr() as *const _,
                bytes.len() as u32,
                None,
            )
        }
        .with_context(|| format!("CreateInMemoryFontFileReference({label}) failed"))?;

        // SAFETY: `file` owned here; `AddFontFile` refcounts internally.
        unsafe { builder.AddFontFile(&file) }
            .with_context(|| format!("FontSetBuilder::AddFontFile({label}) failed"))?;
    }

    // SAFETY: builder valid; CreateFontSet is the terminal op.
    let set: IDWriteFontSet = unsafe { builder.CreateFontSet() }.context("CreateFontSet failed")?;

    // SAFETY: set valid. CreateFontCollectionFromFontSet returns an
    // IDWriteFontCollection1 which inherits IDWriteFontCollection.
    let collection = unsafe { factory5.CreateFontCollectionFromFontSet(&set) }
        .context("CreateFontCollectionFromFontSet failed")?;

    let collection: IDWriteFontCollection = collection
        .cast::<IDWriteFontCollection>()
        .unwrap_or_else(|_| {
            // This can't fail — IDWriteFontCollection1 inherits from
            // IDWriteFontCollection — but use unwrap_or_else to avoid
            // introducing an Err path.
            unreachable!("IDWriteFontCollection1 → IDWriteFontCollection cast")
        });

    // Sanity-check: verify DWrite can actually find BUNDLED_FONT_FAMILY
    // in the collection. If this logs "not found", either the name table
    // inside the TTF doesn't claim that exact family string, or the
    // collection-build path didn't register the file. Either way the
    // render pipeline will silently resolve to a system font downstream
    // and cells will be sized for Segoe UI (hence the visible "wide
    // cells" regression we've chased before). Surfacing it at init makes
    // the root cause obvious in logs.
    let family_w: Vec<u16> = BUNDLED_FONT_FAMILY
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut index: u32 = 0;
    let mut exists = windows::core::BOOL(0);
    // SAFETY: family_w is NUL-terminated; out params are stack-local.
    let find_hr = unsafe {
        collection.FindFamilyName(
            windows::core::PCWSTR(family_w.as_ptr()),
            &mut index,
            &mut exists,
        )
    };

    let total_families = unsafe { collection.GetFontFamilyCount() };
    match find_hr {
        Ok(()) if exists.as_bool() => {
            log::info!(
                "IoskeleyMono bundled collection ready: {total_families} \
                 famil{y_plural}, '{BUNDLED_FONT_FAMILY}' at index {index} \
                 (4 TTF weights registered)",
                y_plural = if total_families == 1 { "y" } else { "ies" },
            );
        }
        Ok(()) => {
            log::warn!(
                "IoskeleyMono bundled collection built with {total_families} \
                 famil{y_plural} but '{BUNDLED_FONT_FAMILY}' NOT found — name \
                 table mismatch; downstream text will resolve to a system \
                 font and cells will be sized for that font's 'M' advance",
                y_plural = if total_families == 1 { "y" } else { "ies" },
            );
        }
        Err(err) => {
            log::warn!(
                "IoskeleyMono bundled collection built but FindFamilyName \
                 failed: {err:?}"
            );
        }
    }

    Ok(Some(collection))
}

/// Return the OS-default [`IDWriteFontFallback`]. The system fallback
/// already knows to cascade through Segoe UI Emoji, Segoe UI Symbol,
/// Segoe UI (Han + Hiragana + Hangul), and the default sans-serif for
/// the active locale — it's the single biggest win for "missing glyph
/// box" bugs and costs zero extra font bytes.
///
/// Returns `None` on pre-Windows-8.1 hosts where `IDWriteFactory2`
/// isn't available — the caller keeps using the bundled-only format
/// and the fallback boxes stay visible. `log::warn` surfaces that so
/// the regression is obvious in logs.
///
fn system_font_fallback(dwrite: &IDWriteFactory) -> Option<IDWriteFontFallback> {
    let factory2: IDWriteFactory2 = match dwrite.cast() {
        Ok(f) => f,
        Err(err) => {
            log::warn!(
                "system_font_fallback: IDWriteFactory2 not available \
                 ({err:?}); missing glyphs will render as boxes"
            );
            return None;
        }
    };
    // SAFETY: factory2 owned here; the returned fallback is a COM
    // reference we own for the life of the GlyphCache.
    match unsafe { factory2.GetSystemFontFallback() } {
        Ok(fb) => {
            log::info!("system_font_fallback: installed OS default cascade");
            Some(fb)
        }
        Err(err) => {
            log::warn!(
                "GetSystemFontFallback failed ({err:?}); missing \
                 glyphs will render as boxes"
            );
            None
        }
    }
}

/// Build the ordered fallback cascade used by terminal text formats:
/// user-selected installed families, bundled IoskeleyMono for remaining
/// private-use icons, then the Windows system cascade.
pub fn font_fallback(
    dwrite: &IDWriteFactory,
    bundled_collection: Option<&IDWriteFontCollection>,
    preferred_families: &[String],
) -> Option<IDWriteFontFallback> {
    let system_fallback = system_font_fallback(dwrite);
    if preferred_families.is_empty() && bundled_collection.is_none() {
        return system_fallback;
    }

    let factory2: IDWriteFactory2 = dwrite.cast().ok()?;
    let builder = match unsafe { factory2.CreateFontFallbackBuilder() } {
        Ok(builder) => builder,
        Err(err) => {
            log::warn!("CreateFontFallbackBuilder failed ({err:?}); using system fallback");
            return system_fallback;
        }
    };
    let null = PCWSTR::null();

    // DirectWrite evaluates matching mappings in builder order. Put the
    // configured families first so this really is an ordered user cascade.
    if !preferred_families.is_empty() {
        let mut system_collection = None;
        if let Err(err) = unsafe { dwrite.GetSystemFontCollection(&mut system_collection, false) } {
            log::warn!("GetSystemFontCollection for preferred fallbacks failed: {err:?}");
        } else if let Some(collection) = system_collection {
            for family in preferred_families {
                let encoded = family
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>();
                let mut family_index = 0;
                let mut exists = windows::core::BOOL::default();
                if let Err(err) = unsafe {
                    collection.FindFamilyName(
                        PCWSTR(encoded.as_ptr()),
                        &mut family_index,
                        &mut exists,
                    )
                } {
                    log::warn!("failed to find preferred fallback {family:?}: {err:?}");
                    continue;
                }
                if !exists.as_bool() {
                    log::warn!("preferred fallback font is not installed: {family:?}");
                    continue;
                }
                let result = (|| -> windows::core::Result<Vec<DWRITE_UNICODE_RANGE>> {
                    let font_family = unsafe { collection.GetFontFamily(family_index)? };
                    let font = unsafe {
                        font_family.GetFirstMatchingFont(
                            DWRITE_FONT_WEIGHT_NORMAL,
                            DWRITE_FONT_STRETCH_NORMAL,
                            DWRITE_FONT_STYLE_NORMAL,
                        )?
                    };
                    let font: IDWriteFont1 = font.cast()?;
                    unicode_ranges_for_font(&font)
                })();
                let ranges = match result {
                    Ok(ranges) if !ranges.is_empty() => ranges,
                    Ok(_) => {
                        log::warn!("preferred fallback font has no Unicode ranges: {family:?}");
                        continue;
                    }
                    Err(err) => {
                        log::warn!("failed to inspect preferred fallback {family:?}: {err:?}");
                        continue;
                    }
                };
                let names = [encoded.as_ptr()];
                if let Err(err) =
                    unsafe { builder.AddMapping(&ranges, &names, &collection, null, null, 1.0) }
                {
                    log::warn!("failed to add preferred fallback {family:?}: {err:?}");
                }
            }
        }
    }

    // Prompt icons use Unicode private-use ranges. Append Con's bundled Nerd
    // Font after the user's choices, but before the Windows system cascade.
    if let Some(collection) = bundled_collection {
        let ranges = [
            DWRITE_UNICODE_RANGE {
                first: 0xE000,
                last: 0xF8FF,
            },
            DWRITE_UNICODE_RANGE {
                first: 0xF0000,
                last: 0xFFFFD,
            },
            DWRITE_UNICODE_RANGE {
                first: 0x100000,
                last: 0x10FFFD,
            },
        ];
        let family: Vec<u16> = BUNDLED_FONT_FAMILY
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let names = [family.as_ptr()];
        if let Err(err) =
            unsafe { builder.AddMapping(&ranges, &names, collection, null, null, 1.0) }
        {
            log::warn!("failed to add bundled PUA fallback mapping: {err:?}");
        }
    }

    if let Some(system_fallback) = system_fallback.as_ref()
        && let Err(err) = unsafe { builder.AddMappings(system_fallback) }
    {
        log::warn!("failed to append system font fallback: {err:?}");
    }

    match unsafe { builder.CreateFontFallback() } {
        Ok(fallback) => {
            log::info!(
                "font fallback cascade ready: preferred={:?}, bundled_pua={}, system={}",
                preferred_families,
                bundled_collection.is_some(),
                system_fallback.is_some()
            );
            Some(fallback)
        }
        Err(err) => {
            log::warn!("CreateFontFallback failed ({err:?}); using system fallback");
            system_fallback
        }
    }
}

fn unicode_ranges_for_font(
    font: &IDWriteFont1,
) -> windows::core::Result<Vec<DWRITE_UNICODE_RANGE>> {
    query_unicode_ranges(|ranges, count| unsafe { font.GetUnicodeRanges(ranges, count) })
}

/// Execute DirectWrite's two-call Unicode-range query.
///
/// The sizing call deliberately supplies no buffer. For any font with at
/// least one range, DirectWrite reports the required count and returns
/// `E_NOT_SUFFICIENT_BUFFER`; that HRESULT is part of the successful sizing
/// contract rather than a reason to discard the preferred fallback.
fn query_unicode_ranges(
    mut get_ranges: impl FnMut(
        Option<&mut [DWRITE_UNICODE_RANGE]>,
        &mut u32,
    ) -> windows::core::Result<()>,
) -> windows::core::Result<Vec<DWRITE_UNICODE_RANGE>> {
    let mut range_count = 0;
    if let Err(err) = get_ranges(None, &mut range_count)
        && err.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(err);
    }
    if range_count == 0 {
        return Ok(Vec::new());
    }

    let mut ranges = vec![DWRITE_UNICODE_RANGE::default(); range_count as usize];
    get_ranges(Some(&mut ranges), &mut range_count)?;
    ranges.truncate(range_count as usize);
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::{DWRITE_UNICODE_RANGE, query_unicode_ranges};
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

    #[test]
    fn unicode_range_query_treats_sizing_error_as_expected() {
        let expected = [
            DWRITE_UNICODE_RANGE {
                first: 0x4E00,
                last: 0x9FFF,
            },
            DWRITE_UNICODE_RANGE {
                first: 0xE000,
                last: 0xF8FF,
            },
        ];
        let mut calls = 0;
        let ranges = query_unicode_ranges(|buffer, actual_count| {
            calls += 1;
            *actual_count = expected.len() as u32;
            let Some(buffer) = buffer else {
                return Err(ERROR_INSUFFICIENT_BUFFER.to_hresult().into());
            };
            buffer.copy_from_slice(&expected);
            Ok(())
        })
        .expect("the expected sizing HRESULT should not abort the query");

        assert_eq!(calls, 2);
        assert_eq!(ranges.len(), expected.len());
        assert_eq!(ranges[0].first, 0x4E00);
        assert_eq!(ranges[1].last, 0xF8FF);
    }
}
