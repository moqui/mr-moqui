# Barcode and GS1 helpers

Server scripts use `ec.resource.barcodeSvg(symbology, text, options)` to render an existing identifier as inline SVG.
Supported names are `Code128`, `EAN-13`, `UPC-A`, `EAN-8`, `ITF-14`, `GS1-128`, `QR` and `DataMatrix`. Retail codes
must include their valid check digit. The helper throws for invalid data or an unsupported name; a batch caller logs
the identifier and continues with its other rows. The older `code128Svg` keeps its log-and-empty behavior.

```js
const svg = ec.resource.barcodeSvg('EAN-13', '4006381333931', {
    height: '22mm', moduleWidth: '0.33mm', quietZone: 11, humanReadable: true
})
```

Options follow `code128Svg`: `height`, `moduleWidth`, `fontSize`, `fontFamily`, `humanReadable` and `quietZone`.
Matrix SVGs use `moduleWidth` and `quietZone`; a layout places human-readable text beside the matrix. Keep at least
four quiet modules around QR and one around Data Matrix. ITF-14 includes horizontal bearer bars. A label template
must choose physical dimensions suitable for its printer and symbology; the helper does not choose a label stock.
QR uses Model 2 with medium error correction; Data Matrix uses ECC 200. The encoders are
[qrcodegen](https://docs.rs/qrcodegen/1.8.0/qrcodegen/) and [datamatrix](https://docs.rs/datamatrix/0.3.3/datamatrix/).

`ec.resource.isValidGs1CheckDigit(text)` checks the GS1 mod-10 digit without changing the identifier. Callers also
check the length appropriate to the identifier type.

`ec.resource.parseGs1(input)` reads an inventory element string, optionally prefixed by AIM `]C1`, `]d2` or `]Q3`,
with ASCII GS (`\u001d`) separating variable fields, or the parenthesized human-readable form:

```js
const scan = ec.resource.parseGs1('(01)04006381333931(17)261231(10)LOT-7(21)SN-9')
scan.applicationIdentifiers['01'] // '04006381333931'
scan.applicationIdentifiers['10'] // 'LOT-7'
scan.elementString              // the canonical scanner payload, including GS separators
scan.humanReadable              // the parenthesized form
```

The result also has `elements`, an ordered list of `{applicationIdentifier, value}`. Supported identifiers are 00,
01, 02, 10, 11, 13, 15, 16, 17, 20, 21, 22, 30, 37, 240, 241, 250, 251, 400, 401, 410–417 and the 310n–316n/320n
measurement families. Lengths, numeric fields, identity check digits and calendar dates are checked. Date day `00`
means the month's last day. Date values stay in GS1 YYMMDD form so the consuming operation can apply its date window.
An unknown identifier, duplicate field or invalid value is an error. A variable field extends to GS or the end;
the parser never guesses another identifier inside a lot or serial value. The field reference is the
[GS1 application identifier registry](https://ref.gs1.org/ai/).

`GS1-128` takes either parsed input form and writes FNC1 at the start and at variable-field boundaries. Product,
pack, lot and stock lookups belong to the consuming component; the framework parser contains no business entity
or application checks.
