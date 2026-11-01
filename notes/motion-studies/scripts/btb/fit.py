import numpy as np
def bez(x1,y1,x2,y2,x):
    s=np.linspace(0,1,2001)
    bx=3*(1-s)**2*s*x1+3*(1-s)*s**2*x2+s**3
    by=3*(1-s)**2*s*y1+3*(1-s)*s**2*y2+s**3
    return np.interp(x,bx,by)
curves={
 # name: (frames offsets from start, values)  start value and end value given
 "is_ring_diameter":(list(range(0,15)),[56,133,171,197,215,229,239,249,255,261,265,271,273,275,278],56,285),
 "tube_ball_y":(list(range(0,33)),[872,732,639,496,411,379,329,308,273,257,232,209,199,181,173,157,150,136,124,118,108,102,93,89,81,73,70,64,62,57,55,51,49],872,49),
 "wipe_edge_x":(list(range(0,9)),[699,677,619,516,301,159,81,28,0],699,0),
 "blackline_disc_top":(list(range(0,7)),[300,202,135,111,101,98,97],300,97),
 "petal_burst_r":(list(range(0,6)),[0,44,140,259,375,450],0,450),
}
for name,(t,v,a,b) in curves.items():
    t=np.array(t,float); y=(np.array(v,float)-a)/(b-a)
    T=t[-1]
    print("==",name,"progress:",np.round(y,3).tolist())
    vel=np.diff(y); print("   per-frame velocity (fraction/frame):",np.round(vel,3).tolist(),"peak at frame",int(np.argmax(vel)),"->",int(np.argmax(vel))+1)
    best=[]
    for x1 in np.linspace(0,1,21):
      for y1 in np.linspace(0,1.6,33):
        for x2 in np.linspace(0,1,21):
          for y2 in np.linspace(0,1.2,25):
            e=np.sqrt(np.mean((bez(x1,y1,x2,y2,t/T)-y)**2)); best.append((e,x1,y1,x2,y2))
    best.sort(); e,x1,y1,x2,y2=best[0]
    print(f"   best cubic-bezier({x1:.2f},{y1:.2f},{x2:.2f},{y2:.2f}) over {int(T)} frames rms {e:.3f}")
    # exponential 1-exp(-t/tau)
    bt=min(((np.sqrt(np.mean((1-np.exp(-t/tau)-y)**2)),tau) for tau in np.linspace(0.5,20,400)))
    print(f"   exp-decay tau={bt[1]:.2f} frames rms {bt[0]:.3f}")
    bp=min(((np.sqrt(np.mean((1-(1-np.clip(t/TT,0,1))**k-y)**2)),k,TT) for k in np.linspace(1,8,71) for TT in np.arange(max(2,T-6),T*2+1)))
    print(f"   power-out k={bp[1]:.1f} T={bp[2]:.0f} frames rms {bp[0]:.3f}")
