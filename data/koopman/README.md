# Koopman CRC Zoo snapshots

Source: Philip Koopman, Carnegie Mellon University,
<https://users.ece.cmu.edu/~koopman/crc/> and its CRC Zoo notes.
The original data is licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
The HTML files in `sources/` retain their copyright notices. Each adjacent JSON
file records its URL, retrieval time and SHA-256 digest.

`catalogue.json` is a derived, normalized catalogue. This transformation converts
implicit +1 notation `k` into `(k << 1 | 1) & ((1 << width) - 1)`. Both reciprocal
forms are retained, including when their software cost differs. All raw source
rows and HD tokens are preserved. HD entries start at HD=3 and refer to **dataword
bits**, excluding CRC bits. Unqualified decimal entries are exact; plain `>=N`
entries are lower bounds; any question-mark annotations remain unknown and never
pass a confirmed-quality filter. Unknown does not mean zero.

The 64-bit page is explicitly under construction. These snapshots are dated
evidence, not an assertion that the published search was exhaustive or that all
values are final. This tool does not recompute Hamming distances or claim error
correction behavior; HD=d implies detection of up to d-1 bit flips in the stated
length range. Source updates can change the catalogue and therefore the run ID.

Refresh explicitly with `python3 scripts/import_koopman.py --refresh`, or rebuild
from saved HTML with `--offline`. Consult the original site before selecting a
polynomial for deployment. Search quality and simplification cost are separate.
