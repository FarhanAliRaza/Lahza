import sys, numpy as np, librosa
y, sr = librosa.load(sys.argv[1], sr=22050); hop=110
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
t=np.arange(len(oenv))*hop/sr
N=876
a=np.zeros(N)
for k in range(N):
    m=(t>=k*0.04-0.02)&(t<k*0.04+0.02)
    a[k]=oenv[m].max() if m.any() else 0
v=np.zeros(N); cutv=np.zeros(N)
for line in open(sys.argv[2]):
    i,tt,df,hd=line.split(); i=int(i); v[i]=float(df)
cuts=[float(x) for x in sys.argv[3].split(',')]
for c in cuts: cutv[int(round(c/0.04))]=1
# visual change: positive derivative of diff to emphasise onsets of motion
vd=np.maximum(0,np.diff(v,prepend=0))
def zs(x): return (x-x.mean())/x.std()
A=zs(a); 
print("lag(frames, + = visual after audio)  corr(diff)  corr(diff-onset)  mean audio onset at cuts (z)")
for L in range(-8,9):
    # visual at k corresponds to audio at k-L
    if L>=0: x,yv,yd,yc=A[:N-L],zs(v)[L:],zs(vd)[L:],cutv[L:]
    else: x,yv,yd,yc=A[-L:],zs(v)[:N+L],zs(vd)[:N+L],cutv[:N+L]
    print(f"{L:+3d}  {np.mean(x*yv):.3f}  {np.mean(x*yd):.3f}  {x[yc>0].mean():+.2f}")
np.savetxt(sys.argv[4],a,fmt="%.3f")
