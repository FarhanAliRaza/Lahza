import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
hop=128
o=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
t=np.arange(len(o))*hop/sr
# global search over period/phase maximizing summed onset strength at grid points
best=[]
for P in np.arange(0.60,0.72,0.0005):
    for ph in np.arange(0,P,0.005):
        g=np.arange(ph,t[-1],P); idx=np.round(g*sr/hop).astype(int); idx=idx[idx<len(o)]
        best.append((o[idx].mean(),P,ph))
best.sort(reverse=True)
print('top grid', [(round(s,3),round(P,4),round(ph,3)) for s,P,ph in best[:5]])
s,P,ph=best[0]
print('BPM',60/P,'frames/beat',P*25)
# windowed check: per 8 beats, best local phase offset
for k in range(0,int(t[-1]/P)-8,8):
    offs=np.arange(-0.1,0.1,0.005); sc=[]
    for d in offs:
        g=ph+P*np.arange(k,k+8)+d; idx=np.round(g*sr/hop).astype(int); sc.append(o[idx].mean())
    print('beats',k,'t=%.2f'%(ph+k*P),'best offset %.3f'%offs[int(np.argmax(sc))])
