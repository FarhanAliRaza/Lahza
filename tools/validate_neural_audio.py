#!/usr/bin/env python3
"""Deterministic local speech-restoration fixture preparation and measurements.

Requires numpy, scipy and soundfile. Public fixtures/provenance live under
ignore/audio-enhancement-analysis/fixtures, not application recordings.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import numpy as np
import soundfile as sf
from scipy import signal


def sisdr(reference, estimate):
    reference = reference.astype(np.float64) - np.mean(reference)
    estimate = estimate.astype(np.float64) - np.mean(estimate)
    target = reference * (np.dot(estimate, reference) / max(np.dot(reference, reference), 1e-20))
    return float(10 * np.log10(max(np.dot(target, target), 1e-20) / max(np.sum((estimate-target)**2), 1e-20)))


def evaluate(reference, noisy, enhanced, sample_rate):
    length=min(len(reference),len(noisy),len(enhanced))
    r,n,e=(x[:length] for x in (reference,noisy,enhanced))
    # Always retain the unshifted metric and separately report delay compensation.
    corr=signal.correlate(e,r,mode='full',method='fft')
    lags=signal.correlation_lags(len(e),len(r),mode='full')
    selected=np.abs(lags)<=int(sample_rate*.1)
    lag=int(lags[selected][np.argmax(corr[selected])])
    aligned_r,aligned_e=(r[:length-lag],e[lag:]) if lag>=0 else (r[-lag:],e[:length+lag])
    freqs=np.fft.rfftfreq(length,1/sample_rate)
    spectrum=np.abs(np.fft.rfft(e))**2
    low=np.sum(spectrum[(freqs>=300)&(freqs<3400)])
    high=np.sum(spectrum[(freqs>=8000)&(freqs<20000)])
    # A fixed filter changes phase, so waveform SI-SDR alone is insufficient
    # for judging a preset that intentionally applies EQ and leveling. Record
    # speech-envelope behavior separately to expose pumping or missing speech.
    block = max(1, round(sample_rate * .02))
    usable = length // block * block
    envelope = {}
    if usable:
        rr = np.sqrt(np.mean(r[:usable].astype(np.float64).reshape(-1, block)**2, axis=1))
        er = np.sqrt(np.mean(e[:usable].astype(np.float64).reshape(-1, block)**2, axis=1))
        active = rr > max(float(np.max(rr)) * .15, 1e-9)
        if np.std(rr) > 1e-12 and np.std(er) > 1e-12:
            envelope['speech_envelope_correlation'] = float(np.corrcoef(rr, er)[0, 1])
        if np.any(active):
            gains = er[active] / np.maximum(rr[active], 1e-12)
            relative = 20 * np.log10(np.maximum(gains / max(float(np.median(gains)), 1e-12), 1e-12))
            envelope['active_gain_spread_db_p10_p90'] = np.percentile(relative, [10, 90]).tolist()
    return {**envelope, 'samples_compared':length,'sample_rate':sample_rate,
        'input_si_sdr_db':sisdr(r,n),'output_si_sdr_db':sisdr(r,e),
        'si_sdr_improvement_db':sisdr(r,e)-sisdr(r,n),
        'estimated_lag_samples':lag,'delay_compensated_si_sdr_db':sisdr(aligned_r,aligned_e),
        'delay_compensated_samples':len(aligned_r),'peak':float(np.max(np.abs(e))),
        'nonfinite_samples':int(np.count_nonzero(~np.isfinite(e))),
        'high_to_voice_band_db':float(10*np.log10(max(high,1e-20)/max(low,1e-20)))}


def prepare(root):
    rows=[]
    for i,p in enumerate(sorted(root.glob('*-clean.wav'))):
        clean,rate=sf.read(p,dtype='float32')
        if clean.ndim!=1: raise ValueError('fixture must be mono')
        rng=np.random.default_rng(700+i)
        t=np.arange(len(clean))/rate
        # Broadband hiss + low-frequency fan and intermittent typing-like
        # impulses are generated mathematically; no external noise recording.
        noise=rng.standard_normal(len(clean))*.25
        noise+=.8*np.sin(2*np.pi*120*t)+.25*np.sin(2*np.pi*240*t)
        for start in np.arange(.15,len(clean)/rate,.31):
            n=min(int(rate*.025),len(clean)-int(start*rate))
            if n>0:noise[int(start*rate):int(start*rate)+n]+=rng.standard_normal(n)*np.exp(-np.arange(n)/(rate*.005))*5
        active=np.abs(clean)>max(np.max(np.abs(clean))*.04,1e-5)
        power=float(np.mean(clean[active]**2))
        noise*=np.sqrt(power/(10**(.5)*np.mean(noise[active]**2)))
        peak=max(np.max(np.abs(clean+noise)),np.max(np.abs(clean)))
        gain=min(1.,.90/max(peak,1e-6))
        reference=clean*gain;mix=(clean+noise)*gain
        stem=p.name.removesuffix('-clean.wav')
        reference_path=root/f'{stem}-reference.wav';noisy=root/f'{stem}-noisy.wav';baseline=root/f'{stem}-afftdn.wav'
        sf.write(reference_path,reference,rate,subtype='FLOAT')
        sf.write(noisy,mix,rate,subtype='FLOAT')
        subprocess.run(['ffmpeg','-v','error','-y','-i',str(noisy),'-af','afftdn=nr=20:tn=1','-c:a','pcm_f32le',str(baseline)],check=True)
        cleaned,_=sf.read(baseline,dtype='float32')
        rows.append({'sample':stem,'baseline':evaluate(reference,mix,cleaned,rate),'noise_seed':700+i,
            'reference_sha256':hashlib.sha256(reference_path.read_bytes()).hexdigest(),
            'noisy_sha256':hashlib.sha256(noisy.read_bytes()).hexdigest()})
    (root/'synthetic-noise-baseline.json').write_text(json.dumps(rows,indent=2)+'\n')
    print(json.dumps(rows,indent=2))


def main():
    p=argparse.ArgumentParser(description=__doc__)
    sub=p.add_subparsers(dest='command',required=True)
    make=sub.add_parser('prepare');make.add_argument('directory',type=Path)
    measure=sub.add_parser('measure');measure.add_argument('reference',type=Path);measure.add_argument('noisy',type=Path);measure.add_argument('enhanced',type=Path);measure.add_argument('--output',type=Path);measure.add_argument('--elapsed',type=float)
    args=p.parse_args()
    if args.command=='prepare':prepare(args.directory);return
    arrays=[];rates=[]
    for path in (args.reference,args.noisy,args.enhanced):
        x,rate=sf.read(path,dtype='float32');arrays.append(x.mean(axis=1) if x.ndim==2 else x);rates.append(rate)
    if len(set(rates))!=1:raise ValueError('sample rates must match')
    result=evaluate(*arrays,rates[0]);result['output_samples']=len(arrays[-1]);result['reference_samples']=len(arrays[0])
    if args.elapsed is not None:result.update(elapsed_seconds=args.elapsed,realtime_factor=args.elapsed/(len(arrays[0])/rates[0]))
    output=json.dumps(result,indent=2)+'\n'
    if args.output:args.output.write_text(output)
    print(output,end='')

if __name__=='__main__':main()
