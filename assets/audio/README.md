# Speech enhancement models

The Enhance audio preset uses NVIDIA NemotronLabs RE-USE on the GPU,
documented with its validation in [RE_USE.md](RE_USE.md). It is the only bundled
speech model.

The public speech is from the CSTR VCTK corpus by Junichi Yamagishi, Christophe
Veaux and Kirsten MacDonald (2019), shared under CC BY 4.0. The fixture manifest
retains the [corpus attribution](https://doi.org/10.7488/ds/2645),
[license evidence](https://datashare.ed.ac.uk/items/30e7453c-9ea8-48b4-8e18-f96d0dc62928/full),
exact demo URLs and SHA-256 hashes, and descriptions of local modifications.
Speech fixtures are developer validation inputs and are not bundled into Lahza.

Native parity, speech measurements, CPU cost, and validation limits are recorded
in [NEURAL_AUDIO_VALIDATION.md](../../docs/NEURAL_AUDIO_VALIDATION.md).
