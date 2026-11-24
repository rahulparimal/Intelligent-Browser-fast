corpus: 3000 documents, 11000 passages; FTS5 index built in 1.36 s
embedded 11000 passages (hashing, 384 dims, int8) in 1.22 s
vector memory held: 4.2 MiB for 11000 vectors

### exact words (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.60 ms | 0.89 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 0.71 ms | 0.90 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 4.90 ms | 7.69 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 5.15 ms | 5.57 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 5.74 ms | 6.18 ms |

### typos (one letter dropped or swapped per word) (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 0.003 | 0.003 | 0.003 | 0 | 0.57 ms | 0.64 ms |
| keyword + fuzzy words | 0.987 | 0.983 | 0.997 | 0 | 30.30 ms | 41.26 ms |
| dense (hashing) | 0.024 | 0.016 | 0.050 | 0 | 4.94 ms | 7.54 ms |
| hybrid (RRF) | 0.027 | 0.020 | 0.053 | 0 | 5.16 ms | 5.78 ms |
| hybrid + fuzzy + rerank | 0.928 | 0.906 | 0.993 | 0 | 34.83 ms | 53.63 ms |

### different word endings (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.60 ms | 0.67 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 0.68 ms | 0.75 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 4.96 ms | 5.83 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 5.16 ms | 5.43 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 5.75 ms | 6.22 ms |

### access control: 200 probes for restricted documents by a caller without access

- keyword (BM25): 0 leaks
- keyword + fuzzy words: 0 leaks
- dense (hashing): 0 leaks
- hybrid (RRF): 0 leaks
- hybrid + fuzzy + rerank: 0 leaks
