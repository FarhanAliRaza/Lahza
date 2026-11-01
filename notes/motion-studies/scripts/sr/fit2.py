import numpy as np, runpy
src=open(__file__.replace('fit2.py','fit.py')).read(); src=src[:src.index('def bez')]
ns={}; exec(src,ns); data=ns['data']
def bez(x1,y1,x2,y2,n=2000):
    t=np.linspace(0,1,n); bx=3*(1-t)**2*t*x1+3*(1-t)*t**2*x2+t**3; by=3*(1-t)**2*t*y1+3*(1-t)*t**2*y2+t**3
    return lambda u: np.interp(u,bx,by)
named={'easeOutCubic':bez(.33,1,.68,1),'easeOutQuart':bez(.25,1,.5,1),'easeOutQuint':bez(.22,1,.36,1),'easeOutExpo':bez(.16,1,.3,1),
'easeOutCirc':bez(0,.55,.45,1),'AE easyEase':bez(.33,0,.67,1),'easeInOutCubic':bez(.65,0,.35,1),'easeInOutSine':bez(.37,0,.63,1),
'easeInCubic':bez(.32,0,.67,0),'easeInQuad':bez(.11,0,.5,0),'AE linOut+90%in':bez(.33,.33,.1,1),'AE 0out+100%in(=0,0,0,1)':bez(0,0,0,1),
'linear':lambda u:u,'expDecay':None}
for k,v in data.items():
    v=np.array(v,float); n=len(v); res=[]
    for name,f in named.items():
        best=(1e18,0,0)
        for D in range(max(2,n-4), n+20):
            for s0 in range(0,8):
                if name=='expDecay': continue
                ts=np.clip((s0+np.arange(n))/D,0,1); Y=f(ts)
                A=np.c_[np.ones(n),Y]; coef=np.linalg.lstsq(A,v,rcond=None)[0]; e=((A@coef-v)**2).sum()
                # require endpoints sensible: the curve must reach end within data (or not) - allow
                if e<best[0]: best=(e,D,s0)
        if name=='expDecay':
            for r in np.arange(0.3,0.97,0.01):
                Y=r**np.arange(n); A=np.c_[np.ones(n),Y]; coef=np.linalg.lstsq(A,v,rcond=None)[0]; e=((A@coef-v)**2).sum()
                if e<best[0]: best=(e,round(r,2),0)
        res.append((np.sqrt(best[0]/n),name,best[1],best[2]))
    res.sort()
    print(k, ' | '.join(f"{nm} D={D} ph={ph} rms={r:.1f}" for r,nm,D,ph in res[:4]))
