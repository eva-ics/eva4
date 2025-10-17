__version__ = '0.1.0'

import evaics.sdk as sdk
import busrt
import queue
import threading
import time

from types import SimpleNamespace
from evaics.sdk import pack, unpack, OID, VideoFrame, RAW_STATE_TOPIC, rpc_e2e

import cv2
import numpy as np
from ultralytics import YOLO

_d = SimpleNamespace(service=None,
                     classes=None,
                     dst_topic=None,
                     debug_topic=None,
                     total_elapsed=0,
                     detector_queue=None,
                     res_queue=None,
                     process_with=None,
                     detector_ready=False,
                     use_keypoints=False,
                     keypoint_colors=[],
                     detector_lock=threading.Lock())

stats = SimpleNamespace(frames_received=0,
                        frames_processed=0,
                        frames_dropped=0,
                        fps=0,
                        last_frame_timestamps=[],
                        t_min=None,
                        t_max=None,
                        t_elapsed=0)


def get_stats():
    return {
        'status':
            'ready' if _d.detector_ready else 'init',
        'frames_processed':
            stats.frames_processed,
        'frames_dropped':
            stats.frames_dropped,
        'fps':
            stats.fps,
        't_min_ms':
            None if stats.t_min is None else int(stats.t_min * 1000),
        't_max_ms':
            None if stats.t_max is None else int(stats.t_max * 1000),
        't_avg_ms':
            None if stats.t_min is None or stats.t_max is None or
            stats.frames_processed == 0 else int(stats.t_elapsed * 1000 /
                                                 stats.frames_processed)
    }


def handle_rpc(event):
    if event.method == b'detector.stats':
        return pack(get_stats())
    else:
        sdk.no_rpc_method()


def on_frame(frame):
    if not _d.service.is_active():
        return
    if not _d.detector_ready:
        announce_noop()
        return
    state = unpack(frame.payload)
    if state['value'] is None:
        announce_noop()
        return
    video_frame = VideoFrame(state['value'])
    stats.frames_received += 1
    locked = _d.detector_lock.acquire(blocking=False)
    if not locked:
        stats.frames_dropped += 1
        return
    try:
        t_started = time.perf_counter()
        _d.detector_queue.put(video_frame)
        bboxes = _d.result_queue.get()
        _d.result_queue.task_done()
        elapsed = time.perf_counter() - t_started
        if stats.t_min is None or stats.t_min > elapsed:
            stats.t_min = elapsed
        if stats.t_max is None or stats.t_max < elapsed:
            stats.t_max = elapsed
        if _d.service.is_active():
            process(bboxes)
        stats.t_elapsed += elapsed
        stats.frames_processed += 1
        stats.last_frame_timestamps.append(time.perf_counter())
        if len(stats.last_frame_timestamps) > 10:
            stats.last_frame_timestamps.pop(0)
            time_between_frames = 0
            for i in range(1, len(stats.last_frame_timestamps)):
                time_between_frames += stats.last_frame_timestamps[
                    i] - stats.last_frame_timestamps[i - 1]
            stats.fps = int(
                (len(stats.last_frame_timestamps) - 1) / time_between_frames)
        event = dict(status=-1 if bboxes is None else 1, value=bboxes)
        if _d.service.is_active():
            if _d.dst_topic:
                _d.service.bus.send(
                    _d.dst_topic,
                    busrt.client.Frame(pack(event), tp=busrt.client.OP_PUBLISH))
    finally:
        _d.detector_lock.release()


def detector(model_path):
    try:
        model = YOLO(model_path, task='detect')
        imgsz = model.model.args['imgsz']
        pixels = np.empty((imgsz, imgsz, 3))
        # warm up
        for _ in range(0, 5):
            model(pixels, verbose=False)
        labels = model.names
        # convert class names to ids
        for c in _d.classes:
            if isinstance(c['id'], str):
                try:
                    c['id'] = next(k for k, v in labels.items() if v == c['id'])
                except:
                    _d.service.logger.warning(f'Unsupported class: {c["id"]}')
        _d.service.logger.info(f'Loaded model: {model_path}')
    except Exception as e:
        _d.service.logger.error(f'Detector was unable to load model: {e}')
        _d.service.mark_terminating()
        return
    _d.detector_ready = True
    debug = _d.debug_topic is not None
    while True:
        video_frame = _d.detector_queue.get()
        try:
            pixels = video_frame.as_numpy_rgb_array()
            if debug:
                out_img = pixels.copy()
            bboxes = []
            res = model(pixels, verbose=False)
            for (i, d) in enumerate(res[0].boxes):
                classidx = int(d.cls.item())
                conf = d.conf.item()
                classname = labels[classidx]
                for c in _d.classes:
                    if c['id'] == classidx and conf >= c['confidence']:
                        xmin, ymin, xmax, ymax = map(int, d.xyxy[0])
                        w = xmax - xmin
                        h = ymax - ymin
                        if w <= 0 or h <= 0:
                            continue
                        color = c['color']
                        bbox = {
                            'x': xmin,
                            'y': ymin,
                            'width': w,
                            'height': h,
                            'color': color,
                            'confidence': conf,
                            'label': classname
                        }
                        if _d.use_keypoints:
                            kp_data = res[0].keypoints[i].data[0]
                            bbox['keypoints'] = kp_data.tolist()
                        bboxes.append(bbox)
                        if debug:
                            cv2.rectangle(out_img, (xmin, ymin), (xmax, ymax),
                                          color, 2)
                            label = f'{classname } {conf:.2f}'
                            labelSize, baseLine = cv2.getTextSize(
                                label, cv2.FONT_HERSHEY_SIMPLEX, 0.5, 1)
                            label_ymin = max(ymin, labelSize[1] + 10)
                            cv2.rectangle(
                                out_img, (xmin, label_ymin - labelSize[1] - 10),
                                (xmin + labelSize[0],
                                 label_ymin + baseLine - 10), color, cv2.FILLED)
                            cv2.putText(out_img, label, (xmin, label_ymin - 7),
                                        cv2.FONT_HERSHEY_SIMPLEX, 0.5,
                                        (0, 0, 0), 1)
                            if _d.use_keypoints:
                                for k, (x, y, _) in enumerate(kp_data):
                                    try:
                                        color = _d.keypoint_colors[k]
                                    except IndexError:
                                        continue
                                    if color is None:
                                        continue
                                    cv2.circle(out_img, (int(x), int(y)), 3,
                                               color, -1)

            if debug:
                video_frame.data = out_img.tobytes()
                event = dict(status=1, value=video_frame.to_bytes())
                _d.service.bus.send(
                    _d.debug_topic,
                    busrt.client.Frame(pack(event), tp=busrt.client.OP_PUBLISH))
        except Exception as e:
            _d.service.logger.error(f'Detector error: {e}')
            bboxes = None
        _d.detector_queue.task_done()
        _d.result_queue.put(bboxes)


def hex_to_rgb(hex_color: str) -> tuple[int, int, int]:
    return tuple(int(hex_color[i:i + 2], 16) for i in (0, 2, 4))


def parse_color(color):
    if color is None:
        return None
    elif color == 'green':
        return (0, 255, 0)
    elif color == 'red':
        return (255, 0, 0)
    elif color == 'blue':
        return (0, 0, 255)
    elif color == 'yellow':
        return (255, 255, 0)
    elif color == 'cyan':
        return (0, 255, 255)
    elif color == 'magenta':
        return (255, 0, 255)
    elif color == 'white':
        return (255, 255, 255)
    elif color == 'black':
        return (0, 0, 0)
    elif color == 'orange':
        return (255, 165, 0)
    elif color == 'purple':
        return (128, 0, 128)
    elif color == 'gray' or color == 'grey':
        return (128, 128, 128)
    elif color.startswith('#'):
        return hex_to_rgb(color[1:])
    else:
        raise ValueError(f'Invalid color: {color}')


def announce_noop():
    with _d.detector_lock:
        payload = pack(dict(status=1, value=None))
        if _d.dst_topic:
            _d.service.bus.send(
                _d.dst_topic,
                busrt.client.Frame(payload, tp=busrt.client.OP_PUBLISH))
        if _d.debug_topic:
            _d.service.bus.send(
                _d.debug_topic,
                busrt.client.Frame(payload, tp=busrt.client.OP_PUBLISH))
        process(None)


def process(bboxes):
    if not _d.process_with:
        return
    timeout = _d.service.timeout['default']
    payload = pack({
        'i': str(_d.process_with),
        'params': {
            'args': [bboxes],
            'kwargs': {
                'detector_stats': get_stats()
            }
        },
        'wait': timeout,
    })
    try:
        res = unpack(
            _d.service.rpc.call('eva.core', busrt.rpc.Request(
                'run', payload)).wait_completed(timeout).get_payload())
        if not res['finished']:
            _d.service.logger.error('Process lmacro call timeout')
        elif res['exitcode'] != 0:
            _d.service.logger.error(
                f'Process lmacro call failed (code {res["exitcode"]}): {res["err"]}'
            )
    except Exception as e:
        _d.service.logger.error(f'Process lmacro call error: {rpc_e2e(e)}')


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='CV Detector',
                           version=__version__)
    info.add_method('detector.stats')
    service = sdk.Service()
    _d.detector_queue = queue.Queue()
    _d.result_queue = queue.Queue()
    _d.service = service
    config = service.get_config()
    _d.classes = config['classes']
    for c in _d.classes:
        if 'id' not in c:
            raise ValueError('class id not specified')
        if not c.get('confidence'):
            raise ValueError('class confidence not specified')
        c['confidence'] = float(c['confidence'])
        color = c.get('color', 'green')
        c['color'] = parse_color(color) or (0, 255, 0)
    if not isinstance(_d.classes, list):
        raise ValueError('classes must be list')
    if 'oid_dst' in config:
        dst_oid = OID(config['oid_dst'])
        _d.dst_topic = f'{RAW_STATE_TOPIC}{dst_oid.to_path()}'
    if 'oid_cv_debug' in config:
        debug_oid = OID(config['oid_cv_debug'])
        _d.debug_topic = f'{RAW_STATE_TOPIC}{debug_oid.to_path()}'
    if 'process' in config:
        _d.process_with = OID(config['process'])
    keypoints = config.get('keypoints', {})
    _d.use_keypoints = keypoints.get('enable', False)
    _d.keypoint_colors = [parse_color(c) for c in keypoints.get('colors', [])]
    service.init(info, on_frame=on_frame, on_rpc_call=handle_rpc)
    service.subscribe_oids([config['oid_src']], event_kind='local')
    service.subscribe_oids([config['oid_src']], event_kind='remote')
    threading.Thread(target=detector, args=(config['model'],),
                     daemon=True).start()
    service.block()
    announce_noop()
