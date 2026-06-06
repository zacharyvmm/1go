# Python Bindings for scah

## Benchmark
### Real Html BenchMark ([html.spec.whatwg.org](https://html.spec.whatwg.org/)) (select all `a` tags):
![WhatWg Html Spec BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/whatwg.png)

| Library | Mean (ms) | stdev | multiplier |
| :--- | :--- | :--- | :--- |
| Scah | **52.203939** | 3.757941 | 1x |
| Selectolax | **143.023167** | 2.674209 | 2.74x |
| lxml | **359.881425** | 5.821705 | 6.89x |
| Parsel | **673.563508** | 5.502256 | 12.9x |
| Gazpacho | **1,637.216892** | 6.151786 | 31.36x |
| BS4 (lxml) | **2,516.724850** | 389.778034 | 48.21x |

### Synthetic Html BenchMark (select all `a` tags):
![Synthetic Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/synthetic.png)

| Library | Mean (ms) | stdev | multiplier |
| :--- | :--- | :--- | :--- |
| Scah | **3.728561** | 0.406445 | 1x |
| Selectolax | **8.066475** | 0.512855 | 2.16x |
| lxml | **24.794239** | 0.229981 | 6.65x |
| Parsel | **76.960953** | 3.008175 | 20.64x |
| BS4 (lxml) | **112.812908** | 2.160341 | 30.26x |
| Gazpacho | **128.430065** | 0.549837 | 34.44x |

### Nested Html BenchMark (select all `Products`):
![Nested Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/nested.png)

| Library | Mean (ms) | stdev | multiplier |
| :--- | :--- | :--- | :--- |
| Scah | **12.470755** | 0.596550 | 1x |
| Selectolax | **66.938622** | 0.498506 | 5.37x |
| Parsel | **301.365925** | 24.076119 | 24.17x |
| lxml | **316.272791** | 2.477959 | 25.36x |
| BS4 (lxml) | **839.663017** | 63.421209 | 67.33x |
