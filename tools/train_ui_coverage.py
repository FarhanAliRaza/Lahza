#!/usr/bin/env python3
"""Train/evaluate a factorial screen-coverage experiment; keep bundled weights intact."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import time

import numpy as np
from PIL import Image, ImageDraw
import torch
import torch.nn.functional as F

import build_ui_coverage_data as generated
import distill_ui_upscaler as distill
import experiment_ui_upscaler_weights as edit
import train_ui_upscaler_v2 as v2
from export_ui_upscaler_v2 import flat, numpy_forward
from ui_coverage_model import CoverageNet, VARIANTS, matched_models

SEED = 2601105


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')


def model_hash(model):
    digest = hashlib.sha256()
    for name, value in sorted(model.state_dict().items()):
        digest.update(name.encode())
        digest.update(str(tuple(value.shape)).encode())
        digest.update(value.detach().cpu().contiguous().numpy().tobytes())
    return digest.hexdigest()


def save_model(model, path):
    temporary = path.with_suffix(path.suffix+'.tmp')
    torch.save({k: v.detach().cpu() for k, v in model.state_dict().items()}, temporary)
    temporary.replace(path)


def generated_sets(bank, prefix):
    target = v2.as_float(torch.from_numpy(bank['hi']))
    return {f'{prefix}-{key}': (v2.as_float(torch.from_numpy(bank[key])), target)
            for key in ['native', 'native_vp8', 'down', 'down_vp8']}


def validation(args):
    result = edit.validation_crops(args.data)
    with np.load(args.generated / 'valid.npz') as z:
        result.update(generated_sets(z, 'outline'))
    return result


@torch.no_grad()
def assess(model, data, device):
    model.eval()
    result = {}
    for label, (x, target) in data.items():
        totals, counts = {}, {}
        for start in range(0, len(x), 4):
            source, truth = x[start:start+4], target[start:start+4]
            pred = model(source.to(device)).float().cpu()
            for key, value in edit.measurements(pred, truth, source).items():
                if value is not None:
                    totals[key] = totals.get(key, 0) + value*len(source)
                    counts[key] = counts.get(key, 0) + len(source)
        row = {k: v/counts[k] for k, v in totals.items()}
        row['psnr'] = float(-10*np.log10(row['mse']))
        result[label] = row
    return result


def synthetic_batch(bank, generator, count):
    device = bank['hi'].device
    ids = torch.randint(len(bank['hi']), (count,), generator=generator).to(device)
    # Independent choice of native/area input and compression for every draw.
    keys = ['native', 'native_vp8', 'down', 'down_vp8']
    kind = torch.randint(4, (count,), generator=generator).to(device)
    x = bank[keys[0]][ids].clone()
    for i, key in enumerate(keys[1:], 1):
        x = torch.where((kind == i)[:, None, None, None], bank[key][ids], x)
    clean = torch.where((kind < 2)[:, None, None, None], bank['native'][ids], bank['down'][ids])
    x, target, clean, alpha = [v2.as_float(a) for a in [x, bank['hi'][ids], clean, bank['alpha'][ids]]]
    colors = bank['colors'][ids].float().div(255)[:, :, None, None]
    invert = (torch.rand(count, 1, 1, 1, generator=generator) < .5).to(device)
    x, target, clean = [torch.where(invert, 1-a, a) for a in [x, target, clean]]
    alpha = torch.where(invert, 1-alpha, alpha)
    colors = torch.where(invert, torch.cat([1-colors[:, 3:], 1-colors[:, :3]], 1), colors)
    perm = torch.randperm(3, generator=generator).to(device)
    return x[:, perm], target[:, perm], clean[:, perm], alpha, torch.cat([colors[:, perm], colors[:, 3+perm]], 1)


def objective(pred, aux, target, clean, alpha, colors, original_count):
    loss = distill.reconstruction(pred, target, clean, edge_weight=1, halo_weight=0)
    if aux:
        a = aux['alpha'][original_count:]
        c = aux['colors'][original_count:]
        loss = loss + .3*(a-alpha).abs().mean() + .1*v2.grad_l1(a, alpha)
        loss = loss + .1*(c-colors).abs().mean()
        loss = loss + .1*(aux['geometry'][original_count:]-target[original_count:]).abs().mean()
        # Train the fallback independently and supervise which branch fits locally.
        loss = loss + .25*distill.reconstruction(aux['rgb'], target, clean, edge_weight=1, halo_weight=0)
        with torch.no_grad():
            rgb_error = (aux['rgb']-target).abs().mean(1, keepdim=True)
            geometry_error = (aux['geometry']-target).abs().mean(1, keepdim=True)
            prefer_geometry = ((rgb_error-geometry_error)/.02).sigmoid()
        loss = loss + .05*F.binary_cross_entropy(aux['gate'].clamp(1e-6, 1-1e-6), prefer_geometry)
    return loss


def train(args):
    if not torch.cuda.is_available():
        raise RuntimeError('CUDA is required for this training experiment')
    torch.manual_seed(args.sampling_seed)
    torch.backends.cudnn.benchmark = True
    device = 'cuda'
    val = validation(args)
    base = assess(edit.baseline().to(device), val, device)
    write_json(args.out/'baseline-validation.json', base)
    sets = []
    for source in ['docs', 'scenes', 'code']:
        with np.load(args.data/f'train-{source}.npz') as z:
            sets.append({k: torch.from_numpy(z[k]).to(device) for k in ['lo', 'hi2', 'hi4', 'codec', 'down2', 'down2codec']})
    with np.load(args.generated/'train.npz') as z:
        bank = {k: torch.from_numpy(z[k]).to(device) for k in z.files}
    models = {k: v.to(device) for k, v in matched_models(args.init_seed).items() if k in args.variants}
    if args.warm_start:
        for name, model in models.items():
            model.load_state_dict(torch.load(args.warm_start/f'{name}.pt', map_location=device, weights_only=True))
    opts = {k: torch.optim.Adam(m.parameters(), lr=args.lr) for k, m in models.items()}
    schedules = {k: torch.optim.lr_scheduler.OneCycleLR(o, max_lr=args.lr, total_steps=args.steps,
                                                      pct_start=max(.05, 2/args.steps)) for k, o in opts.items()}
    best, history = {}, []
    generator = torch.Generator().manual_seed(args.sampling_seed)
    config = {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()}
    first_step, previous_seconds = 1, 0
    if args.resume:
        state = torch.load(args.out/'resume.pt', map_location='cpu', weights_only=True)
        for key in config:
            if key not in ['resume', 'stop_after'] and config[key] != state['config'][key]:
                raise ValueError(f'Resume configuration changed: {key}')
        for name, model in models.items():
            model.load_state_dict(state['models'][name])
            opts[name].load_state_dict(state['optimizers'][name])
            schedules[name].load_state_dict(state['schedulers'][name])
        generator.set_state(state['generator'])
        torch.set_rng_state(state['torch_rng'])
        torch.cuda.set_rng_state_all(state['cuda_rng'])
        first_step, previous_seconds = state['step']+1, state['seconds']
        best, history = state['best'], state['history']
    else:
        for name, model in models.items():
            rows = assess(model, val, device)
            best[name] = distill.score(rows, base)
            save_model(model, args.out/f'{name}.pt')
            write_json(args.out/f'{name}-validation.json', {'step': 0, 'score': best[name], 'rows': rows})
    started = time.monotonic()
    write_json(args.out/'options.json', config)
    print('Training', list(models), 'GPU GB', round(torch.cuda.memory_allocated()/1e9, 2), flush=True)
    last_step = min(args.steps, args.stop_after or args.steps)
    for step in range(first_step, last_step+1):
        original = v2.batch(sets, 2, 48, 24, generator)
        extra = synthetic_batch(bank, generator, 8)
        x, target, clean = [torch.cat([a, b]) for a, b in zip(original, extra[:3])]
        losses = {}
        for name, model in models.items():
            model.train()
            opts[name].zero_grad(set_to_none=True)
            pred, aux = model(x, auxiliary=True)
            loss = objective(pred, aux, target, clean, extra[3], extra[4], 24)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.)
            opts[name].step()
            schedules[name].step()
            if step % 250 == 0:
                losses[name] = round(float(loss.detach()), 5)
        if step % 250 == 0:
            print(step, 'seconds', round(time.monotonic()-started, 1), losses, flush=True)
        if step % 1000 == 0 or step == last_step:
            for name, model in models.items():
                rows = assess(model, val, device)
                value = distill.score(rows, base)
                row = {'name': name, 'step': step, 'score': value, 'rows': rows}
                history.append(row)
                if value < best[name]:
                    best[name] = value
                    save_model(model, args.out/f'{name}.pt')
                    write_json(args.out/f'{name}-validation.json', row)
                print('validation', name, step, round(value, 6), 'best', round(best[name], 6), flush=True)
            write_json(args.out/'history.json', history)
            state = {'step': step, 'seconds': previous_seconds+time.monotonic()-started,
                     'config': config, 'models': {n: m.state_dict() for n, m in models.items()},
                     'optimizers': {n: o.state_dict() for n, o in opts.items()},
                     'schedulers': {n: s.state_dict() for n, s in schedules.items()},
                     'generator': generator.get_state(), 'torch_rng': torch.get_rng_state(),
                     'cuda_rng': torch.cuda.get_rng_state_all(), 'best': best, 'history': history}
            torch.save(state, args.out/'resume.pt.tmp')
            (args.out/'resume.pt.tmp').replace(args.out/'resume.pt')
    for name, model in models.items():
        save_model(model, args.out/f'{name}-last.pt')
    write_json(args.out/'training.json', {
        'seconds': previous_seconds+time.monotonic()-started,
        'seed': args.sampling_seed, 'init_seed': args.init_seed,
        'torch': torch.__version__, 'device': torch.cuda.get_device_name(),
        'precision': 'FP32; TF32 disabled', 'steps': last_step, 'requested_steps': args.steps,
        'warm_start': str(args.warm_start) if args.warm_start else None, 'batch': 32,
        'fresh_original_crops_per_batch': 24, 'supervised_coverage_pairs_per_batch': 8,
        'variants': {n: {'training_parameters': m.parameters_count(),
                         'deployed_parameters': m.fused().parameters_count(),
                         'convolution_macs_per_input_pixel': m.macs_per_input_pixel()} for n, m in models.items()},
        'base_loss': 'Charbonnier + gradient L1 + backprojection L1',
        'coverage_auxiliary_loss': '0.3 alpha L1 + 0.1 alpha gradient + 0.1 color L1 + 0.1 geometry L1 + 0.25 fallback reconstruction + 0.05 detached oracle gate BCE',
        'data_manifest_sha256': hashlib.sha256((args.data/'manifest.json').read_bytes()).hexdigest(),
        'generated_data': json.loads((args.generated/'data.json').read_text()),
    })


def load_models(args, device):
    names = json.loads((args.out/'options.json').read_text())['variants']
    models = {'v2': edit.baseline().to(device)}
    for name in names:
        model = CoverageNet(*VARIANTS[name])
        model.load_state_dict(torch.load(args.out/f'{name}.pt', map_location='cpu', weights_only=True))
        models[name] = model.eval().to(device)
    return models


@torch.no_grad()
def benchmark(model, device, height=270, width=480):
    x = torch.rand(1, 3, height, width, device=device)
    timings = {}
    for precision in ['float32', 'float16']:
        m = copy.deepcopy(model).float() if precision == 'float32' else copy.deepcopy(model).half()
        xx = x.float() if precision == 'float32' else x.half()
        for _ in range(10):
            m(xx)
        torch.cuda.synchronize()
        samples = []
        for _ in range(30):
            start, end = torch.cuda.Event(enable_timing=True), torch.cuda.Event(enable_timing=True)
            start.record()
            m(xx)
            end.record()
            end.synchronize()
            samples.append(start.elapsed_time(end))
        timings[precision] = {'median_ms': float(np.median(samples)), 'p95_ms': float(np.percentile(samples, 95))}
    return timings


@torch.no_grad()
def coverage_metrics(models, bank, device):
    result = {}
    for name, model in models.items():
        rows = []
        for start in range(0, len(bank['hi']), 4):
            x = v2.as_float(torch.from_numpy(bank['native_vp8'][start:start+4])).to(device)
            truth = v2.as_float(torch.from_numpy(bank['alpha'][start:start+4]))
            colors = torch.from_numpy(bank['colors'][start:start+4]).float().div(255)[:, :, None, None]
            if name != 'v2' and model.coverage:
                pred, aux = model(x, auxiliary=True)
                gate = float(aux['gate'].mean())
                learned_mask_mae = float((aux['alpha'].cpu()-truth).abs().mean())
            else:
                pred, gate, learned_mask_mae = model(x), None, None
            # Recover coverage using known test colors, uniformly for every model.
            pred = edit.quantize(pred.float()).cpu()
            direction = colors[:, :3]-colors[:, 3:]
            estimated = (((pred-colors[:, 3:])*direction).sum(1, keepdim=True)/direction.square().sum(1, keepdim=True).clamp_min(1e-6)).clamp(0, 1)
            foreground = truth > .5
            inferred = estimated > .5
            # For sparse synthetic glyphs, use the minority region as an ink proxy.
            invert = foreground.float().mean((1, 2, 3), keepdim=True) > .5
            foreground = foreground ^ invert
            inferred = inferred ^ invert
            false_negative = float((foreground & ~inferred).sum()/foreground.sum().clamp_min(1))
            false_positive = float((~foreground & inferred).sum()/(~foreground).sum().clamp_min(1))
            rows.append({'coverage_mae': float((estimated-truth).abs().mean()),
                         'minority_mask_miss_rate': false_negative, 'minority_mask_false_positive_rate': false_positive,
                         'relative_minority_area_error': float((inferred.sum((1,2,3))-foreground.sum((1,2,3))).abs().float().div(foreground.sum((1,2,3)).clamp_min(1)).mean()),
                         'geometry_gate': gate, 'learned_mask_mae': learned_mask_mae})
        result[name] = {k: float(np.mean([r[k] for r in rows])) if rows[0][k] is not None else None for k in rows[0]}
    return result


@torch.no_grad()
def panel(models, x, truth, path, reference_label='Target'):
    tiles = []
    images = [('Input', F.interpolate(x, scale_factor=2, mode='nearest'))]
    images += [(name, model.cpu()(x)) for name, model in models.items()]
    images += [(reference_label, truth)]
    for label, pred in images:
        a = edit.quantize(pred)[0].permute(1, 2, 0).mul(255).round().byte().numpy()
        im = Image.fromarray(a).resize((a.shape[1]*2, a.shape[0]*2), Image.Resampling.NEAREST)
        tile = Image.new('RGB', (im.width, im.height+24), (245,245,245))
        tile.paste(im, (0,24))
        ImageDraw.Draw(tile).text((6,5), label, fill=(20,20,20))
        tiles.append(tile)
    canvas = Image.new('RGB', (tiles[0].width*4, tiles[0].height*2), (245,245,245))
    for i, tile in enumerate(tiles):
        canvas.paste(tile, ((i%4)*tile.width, (i//4)*tile.height))
    canvas.save(path)


@torch.no_grad()
def evaluate(args):
    device = 'cuda' if torch.cuda.is_available() else 'cpu'
    models = load_models(args, device)
    hashes = {n: model_hash(m) for n, m in models.items()}
    frozen = args.out/'frozen.json'
    if frozen.exists() and json.loads(frozen.read_text()) != hashes:
        raise RuntimeError('Candidates were already frozen with different hashes')
    write_json(frozen, hashes)
    result = {'hashes': hashes, 'sets': {}, 'diagnostics': {}, 'fusion': {}, 'timing': {}}
    examples = []
    # Freeze every checkpoint before opening old or newly generated test data.
    for source in ['docs', 'scenes', 'code']:
        with np.load(args.data/f'test-{source}.npz') as z:
            truth = v2.as_float(torch.from_numpy(z['hi2']))
            for label, key in [('native', 'lo'), ('native-vp8', 'codec'), ('down-vp8', 'down2codec')]:
                a = z[key]
                if key == 'codec':
                    a = a[:, 1]
                    crop = v2.as_float(torch.from_numpy(a[7:8, 24:88, 32:128]))
                    examples.append((source, crop, truth[7:8, :, 48:176, 64:256]))
                pair = {f'{source}-{label}': (v2.as_float(torch.from_numpy(a)), truth)}
                for name, model in models.items():
                    for group, metrics in assess(model, pair, device).items():
                        result['sets'].setdefault(group, {})[name] = metrics
                print('test', source, label, {n: round(v['psnr'], 4) for n, v in result['sets'][f'{source}-{label}'].items()}, flush=True)
                write_json(args.out/'test.json', result)
    for label, cjk in [('outline', False), ('cjk', True)]:
        bank = generated.generate('test', 128 if not cjk else 48, 64, cjk)
        groups = generated_sets(bank, label)
        for name, model in models.items():
            for group, metrics in assess(model, groups, device).items():
                result['sets'].setdefault(group, {})[name] = metrics
        result[f'{label}_coverage'] = coverage_metrics(models, bank, device)
        for index in [1, 7]:
            examples.append((f'{label}-{index}', groups[f'{label}-native_vp8'][0][index:index+1], groups[f'{label}-native_vp8'][1][index:index+1]))
        print('fresh test', label, flush=True)
        write_json(args.out/'test.json', result)
    result['encoded_pan_excess_error_8bit'] = distill.encoded_pan(models, device)
    for name, model in list(models.items()):
        model.cpu()
        result['diagnostics'][name] = edit.diagnostics(model, True)
        result.setdefault('synthetic_pan_error_8bit', {})[name] = edit.motion_error(model)
        if name != 'v2':
            fused = model.fused().eval()
            probe = torch.rand(2, 3, 31, 37, generator=torch.Generator().manual_seed(73))
            error = float((fused(probe)-model(probe)).abs().max())
            assert error < 1e-5
            save_model(fused, args.out/f'{name}-fused.pt')
            result['fusion'][name] = {'max_error': error, 'deployed_parameters': fused.parameters_count(), 'hash': model_hash(fused)}
            if name in ['rgb', 'rgb_edge']:
                weights = flat(fused)
                layout_error = float(np.abs(numpy_forward(weights, probe[0].numpy(), 2, 16, 6)
                                     - fused.tail(edit.features(fused, probe))[0].numpy()).max())
                assert layout_error < 1e-4
                (args.out/f'{name}-2x.f32').write_bytes(weights.tobytes())
                result['fusion'][name].update({'shader_layout_max_error': layout_error,
                                               'flat_sha256': hashlib.sha256(weights.tobytes()).hexdigest()})
            models[name] = fused
        if device == 'cuda':
            before = model_hash(models[name])
            result['timing'][name] = benchmark(models[name].to(device), device)
            models[name].cpu()
            assert model_hash(models[name]) == before, 'Timing must not round or mutate candidate weights'
    frames = sorted((args.data/'real').glob('*.png'))
    if frames:
        result['real_crispness'] = v2.crispness(models, 2, 'cpu', frames)
    review = args.out/'review-final'
    review.mkdir(exist_ok=True)
    order = ['rgb_wide', 'coverage_edge', 'rgb', 'v2', 'coverage', 'rgb_edge']
    order = [name for name in order if name in models]
    anonymous = {chr(65+i): models[name] for i, name in enumerate(order)}
    write_json(review/'mapping.json', dict(zip(anonymous, order)))
    for label, x, truth in examples:
        panel(models, x, truth, args.out/f'compare-{label}.png')
        panel(anonymous, x, truth, review/f'{label}.png')
    for index, path in enumerate(frames[:2]):
        x = v2.as_float(torch.from_numpy(np.asarray(Image.open(path).convert('RGB')).copy()[None]))
        energy = F.avg_pool2d(v2.local_range(x.mean(1, keepdim=True)), 128, 128)[0, 0]
        y, xx = divmod(int(energy.argmax()), energy.shape[1])
        crop = x[..., y*128:(y+1)*128, xx*128:(xx+1)*128]
        bilinear = F.interpolate(crop, scale_factor=2, mode='bilinear', align_corners=False)
        panel(anonymous, crop, bilinear, review/f'real-{index}.png', 'Bilinear; no HR truth')
    write_json(args.out/'test.json', result)
    print('Evaluation and fused candidates saved', flush=True)


@torch.no_grad()
def infer(args):
    coverage, _, channels = VARIANTS[args.model]
    model = CoverageNet(coverage, False, channels).eval()
    model.load_state_dict(torch.load(args.out/f'{args.model}-fused.pt', map_location='cpu', weights_only=True))
    im = Image.open(args.input).convert('RGBA')
    array = np.asarray(im)
    device = 'cuda' if torch.cuda.is_available() else 'cpu'
    model.to(device)
    x = v2.as_float(torch.from_numpy(array[None, :, :, :3].copy()))
    height, width = array.shape[:2]
    result = torch.empty(1, 3, height*2, width*2)
    for y in range(0, height, 192):
        for xx in range(0, width, 192):
            y1, x1 = min(y+192, height), min(xx+192, width)
            sy, sx, ey, ex = max(y-8, 0), max(xx-8, 0), min(y1+8, height), min(x1+8, width)
            pred = model(x[..., sy:ey, sx:ex].to(device)).float().cpu()
            result[..., y*2:y1*2, xx*2:x1*2] = pred[..., (y-sy)*2:(y1-sy)*2, (xx-sx)*2:(x1-sx)*2]
    rgb = edit.quantize(result)[0].permute(1, 2, 0).mul(255).round().byte().numpy()
    alpha = im.getchannel('A').resize((width*2, height*2), Image.Resampling.BILINEAR)
    image = Image.fromarray(rgb)
    image.putalpha(alpha)
    image.save(args.output)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('command', choices=['train', 'evaluate', 'infer'])
    p.add_argument('--out', type=Path, default=Path('ignore/upscaler-coverage'))
    p.add_argument('--data', type=Path, default=v2.ROOT/'ignore/upscaler-data')
    p.add_argument('--generated', type=Path, default=Path('ignore/upscaler-coverage-data'))
    p.add_argument('--steps', type=int, default=20000)
    p.add_argument('--lr', type=float, default=.001)
    p.add_argument('--init-seed', type=int, default=2604)
    p.add_argument('--sampling-seed', type=int, default=SEED)
    p.add_argument('--warm-start', type=Path)
    p.add_argument('--resume', action='store_true')
    p.add_argument('--stop-after', type=int)
    p.add_argument('--variants', nargs='+', choices=list(VARIANTS), default=list(VARIANTS))
    p.add_argument('--model', choices=list(VARIANTS), default='coverage_edge')
    p.add_argument('--input', type=Path)
    p.add_argument('--output', type=Path)
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if args.command == 'train' and (args.out/'frozen.json').exists():
        p.error('Candidates frozen; use a fresh experiment directory')
    if args.command == 'train' and args.steps < 4:
        p.error('Training requires at least four steps for the learning-rate schedule')
    if args.command == 'train' and args.stop_after is not None and args.stop_after < 1:
        p.error('--stop-after must be positive')
    if args.command == 'infer' and (args.input is None or args.output is None):
        p.error('infer requires --input and --output')
    torch.set_num_threads(4)
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    {'train': train, 'evaluate': evaluate, 'infer': infer}[args.command](args)


if __name__ == '__main__':
    main()
