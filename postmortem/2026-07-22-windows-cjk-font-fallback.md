# Windows CJK fallback ignored the configured font

## What happened

Windows terminal users could configure an ordered CJK fallback such as
`等距更纱黑体 SC`, but some Han characters still appeared with unexpected
regional glyph forms. Characters whose simplified-Chinese and other CJK
forms differ visibly made the problem easiest to spot.

## Root cause

The preferred-font mapping queried `IDWriteFont1::GetUnicodeRanges` with no
output buffer to obtain the required range count. DirectWrite correctly
returned `E_NOT_SUFFICIENT_BUFFER` while filling that count, but the code
propagated the HRESULT as an error. The preferred family was consequently
never added to the custom fallback cascade.

The remaining Windows system fallback also received a hard-coded `en-US`
locale from `CreateTextFormat`, so it had no user-specific regional hint when
choosing CJK glyph forms.

## Fix applied

- Treat `E_NOT_SUFFICIENT_BUFFER` as the expected result of the sizing call,
  allocate the reported range buffer, and perform the second query.
- Read the current user's BCP 47 locale with `GetUserDefaultLocaleName` and
  pass it to DirectWrite, falling back to `en-US` only if the API fails.
- Add a unit test that reproduces the sizing-call HRESULT contract.

## What we learned

Win32 two-call buffer APIs do not all report the sizing call as `S_OK`.
Wrapper code must validate each API's documented sizing HRESULT instead of
using unconditional error propagation. Font fallback tests should verify
that a configured family reaches the mapping builder, not only that the
configuration order is preserved.
