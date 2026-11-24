corpus: 3000 documents, 11000 passages; FTS5 index built in 2.99 s
embedded 11000 passages (hashing, 384 dims, int8) in 3.09 s
vector memory held: 4.2 MiB for 11000 vectors

### exact words (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.87 ms | 1.45 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 1.00 ms | 1.85 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 7.69 ms | 10.45 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 7.29 ms | 9.60 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 9.65 ms | 14.10 ms |

### typos (one letter dropped or swapped per word) (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 0.003 | 0.003 | 0.003 | 0 | 0.93 ms | 1.96 ms |
| keyword + fuzzy words | 0.987 | 0.983 | 0.997 | 0 | 10.68 ms | 16.02 ms |
| dense (hashing) | 0.024 | 0.016 | 0.050 | 0 | 6.69 ms | 9.09 ms |
| hybrid (RRF) | 0.027 | 0.020 | 0.053 | 0 | 7.40 ms | 10.25 ms |
| hybrid + fuzzy + rerank | 0.928 | 0.906 | 0.993 | 0 | 18.22 ms | 27.21 ms |

### different word endings (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.90 ms | 1.21 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 1.04 ms | 1.51 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 6.46 ms | 8.43 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 6.95 ms | 9.57 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 7.63 ms | 9.66 ms |

### access control: 200 probes for restricted documents by a caller without access

- keyword (BM25): 0 leaks
- keyword + fuzzy words: 0 leaks
- dense (hashing): 0 leaks
- hybrid (RRF): 0 leaks
- hybrid + fuzzy + rerank: 0 leaks
