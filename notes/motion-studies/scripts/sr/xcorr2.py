import sys, numpy as np
a=np.loadtxt(sys.argv[1]); N=len(a); A=(a-a.mean())/a.std()
cuts=[float(x) for x in sys.argv[2].split(',')]
ci=np.array([int(round(c/0.04)) for c in cuts])
rng=np.random.default_rng(0)
res=[]
for L in range(-50,51):
    idx=ci-L; idx=idx[(idx>=0)&(idx<N)]
    res.append((L,A[idx].mean()))
for L,m in sorted(res,key=lambda r:-r[1])[:8]: print(f"lag {L:+d}f mean z {m:+.2f}")
# null distribution: random cut sets
null=[A[rng.integers(0,N,len(ci))].mean() for _ in range(5000)]
print("null 95th pct", np.percentile(null,95), "99th", np.percentile(null,99))
# with +-1 frame tolerance (max in window)
for L in [0]:
    w=[max(A[max(0,i-1):i+2]) for i in ci]
    print("lag0 window±1 mean z", np.mean(w), " null:", np.percentile([np.mean([max(A[max(0,i-1):i+2]) for i in rng.integers(1,N-1,len(ci))]) for _ in range(2000)],[50,95]))
