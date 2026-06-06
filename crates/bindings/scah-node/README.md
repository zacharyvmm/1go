# Nodejs/Bun bindings for scan HTML

```bash
npm install scah@npm:@zacharymm/scah
```

## Benchmark

### Synthetic Html BenchMark (select all `a` tags):

![Synthetic Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/synthetic.png)

| Library          | Mean (ms)      | stdev    | multiplier |
| :--------------- | :------------- | :------- | :--------- |
| scah             | **5.302260**   | 0.907064 | 1x         |
| linkedom         | **24.410154**  | 3.157122 | 4.6x       |
| cheerio          | **30.391315**  | 1.222545 | 5.73x      |
| happy-dom        | **78.305500**  | 7.789133 | 14.77x     |
| node-html-parser | **103.388125** | 4.417629 | 19.5x      |
| jsdom            | **109.433788** | 8.876794 | 20.64x     |

### WHATWG Html BenchMark (find all `a` tags):

![WHATWG Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/whatwg.png)

| Library          | Mean (ms)      | stdev      | multiplier |
| :--------------- | :------------- | :--------- | :--------- |
| scah             | **57.891190**  | 3.669321   | 1x         |
| linkedom         | **420.146805** | 177.607558 | 7.26x      |
| cheerio          | **540.674035** | 29.571992  | 9.34x      |
| node-html-parser | **707.293309** | 28.933410  | 12.22x     |

### Nested Html BenchMark (find all products)

![Nested Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/nested.png)

| Library          | Mean (ms)      | stdev     | multiplier |
| :--------------- | :------------- | :-------- | :--------- |
| scah             | **27.346216**  | 3.100238  | 1x         |
| linkedom         | **76.700628**  | 8.804752  | 2.8x       |
| cheerio          | **96.956129**  | 5.551734  | 3.55x      |
| node-html-parser | **129.333271** | 19.915217 | 4.73x      |
| jsdom            | **362.924202** | 88.057259 | 13.27x     |
