import sys, numpy as np
on=np.loadtxt(sys.argv[1]); o=on[on[:,1]>0.45,0]
def frac(ts,w): return np.mean([np.min(np.abs(o-t))*25<=w for t in ts])
groove=[2.08,2.88,3.72,4.56,5.64,6.56,7.96,8.96,10.80,11.48,12.24,13.44,14.44,14.88,15.20,16.52,17.52,18.48,19.92,20.60,21.48,22.16,23.40,24.64,25.44,27.00,27.88,28.76]
brk=[0.08,0.60,1.04,29.84,30.36,30.84,31.52,31.84,32.00,32.24,32.40,32.68,32.84,33.16]
rng=np.random.default_rng(1)
for name,ts in (('groove cuts',groove),('break/outro events',brk)):
    for w in (1,2):
        null=[frac(rng.uniform(min(ts),max(ts),len(ts)),w) for _ in range(3000)]
        print(f"{name} n={len(ts)} within±{w}f: {frac(ts,w):.2f}  (random: mean {np.mean(null):.2f}, 95th {np.percentile(null,95):.2f})")
    offs=[(t-o[np.argmin(np.abs(o-t))])*25 for t in ts]
    print("  signed offsets (f):",' '.join(f"{x:+.1f}" for x in offs), " mean|off|", round(np.mean(np.abs(offs)),2))
print("strong onsets/s:", len(o)/35)
