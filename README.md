SAG-RS lets a cluster of your own computers work together as a team of coding and research assistants, using models running locally on your own hardware instead of paying for cloud services. 
It's a Rust rewrite and evolution of an earlier project [SAGIDE](https://github.com/sanjeevakumarh/Structured-Agent-Graph-IDE) keeping the good ideas but making it lighter, faster, and command-line.

Simple enough to set up with two scripts, but easy to take apart and customize as needed. Tested across a real mixe of home cluster: GMK EVO-X2 (Radeon 8060S, ROCm), an Intel Arc Pro B70 (32GB, in a Lenovo P520), and an RTX A4500 (20GB, in an HP Z820) — deliberately spanning AMD, Intel, and NVIDIA backends to prove it stays truly vendor-agnostic.
