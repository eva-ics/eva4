import os
from .tools import can_colorize, safe_print, get_term_size
from neotermcolor import colored


def plot_bar_chart(data):
    BAR = '❚'
    TICK = '▏'

    width, height = get_term_size()
    spaces = 8
    extra = 20 if can_colorize() else 0
    max_label_length = 0
    max_value_label_length = 0
    min_value = None
    max_value = None
    for label, value in data:
        if value is not None:
            if len(label) > max_label_length:
                max_label_length = len(label)
            if len(str(value)) > max_value_label_length:
                max_value_label_length = len(str(value))
            if min_value is None or value < min_value:
                min_value = value
            if max_value is None or value > max_value:
                max_value = value
    if max_value is None:
        max_value = 0
    if min_value is None:
        min_value = 0
    plot_width = width - spaces - max_label_length - max_value_label_length
    if max_value > 0:
        max_bar_len = max_value - min_value
    else:
        max_bar_len = abs(min_value - max_value)
    zoom = plot_width / max_bar_len if max_bar_len else 1
    for label, value in data:
        if value is None:
            line = colored('NaN', color='magenta')
        else:
            if min_value >= 0:
                line = BAR * int((value - min_value) * zoom)
                if not line:
                    line = f'{TICK if value else " "}{value}'
                else:
                    line += f' {value}'
                line = colored(line,
                               color='green' if value is not None else 'grey')
            elif max_value < 0:
                line = BAR * int((abs(max_value - value)) * zoom)
                if not line:
                    line = f'{value} {(TICK) if value else ""}'
                    line = line.rjust(plot_width + spaces + 1)
                else:
                    line = f'{value} ' + line
                    line = line.rjust(plot_width + spaces)
                line = colored(line,
                               color='yellow' if value is not None else 'grey')
            else:
                if value >= 0:
                    line = BAR * int(value * zoom) + f' {value}'
                    line = colored(
                        line, color='green' if value is not None else 'grey')
                    line = ' ' * (1 + int(
                        abs(min_value) * zoom + len(str(min_value)))) + line
                else:
                    bar = BAR * int(abs(value) * zoom)
                    if not bar:
                        bar = TICK
                    line = f'{value} ' + bar
                    line = line.rjust(
                        1 + int(abs(min_value) * zoom + len(str(min_value))))
                    line = colored(line, color='yellow')
        safe_print(
            f'{colored(label.rjust(max_label_length), color="cyan")}'
            f' {line}', extra)


def plot_line_chart(data, rows, max_label_width):
    # https://github.com/kroitor/asciichart/tree/master/asciichartpy
    # Copyright © 2016 Igor Kroitor
    from math import ceil, floor, isnan

    def _isnum(n):
        return not isnan(n)

    def colored(char, color):
        if not color:
            return char
        else:
            return color + char + "\033[0m"

    def _plot(series, cfg=None):
        if len(series) == 0:
            return ''

        if not isinstance(series[0], list):
            if all(isnan(n) for n in series):
                return ''
            else:
                series = [series]

        cfg = cfg or {}
        colors = cfg.get('colors', [None])
        minimum = cfg.get('min',
                          min(filter(_isnum, [j for i in series for j in i])))
        maximum = cfg.get('max',
                          max(filter(_isnum, [j for i in series for j in i])))
        default_symbols = ['┼', '┤', '╶', '╴', '─', '╰', '╭', '╮', '╯', '│']
        symbols = cfg.get('symbols', default_symbols)
        if minimum > maximum:
            raise ValueError('The min value cannot exceed the max value.')
        interval = maximum - minimum
        offset = cfg.get('offset', 3)
        height = cfg.get('height', interval)
        ratio = height / interval if interval > 0 else 1
        min2 = int(floor(minimum * ratio))
        max2 = int(ceil(maximum * ratio))

        def clamp(n):
            return min(max(n, minimum), maximum)

        def scaled(y):
            return int(round(clamp(y) * ratio) - min2)

        rows = max2 - min2

        width = 0
        for i in range(0, len(series)):
            width = max(width, len(series[i]))
        width += offset
        placeholder = cfg.get(
            'format',
            '{:' + str(8 if max_label_width < 8 else max_label_width) + '.2f} ')
        result = [[' '] * width for i in range(rows + 1)]
        # axis and labels
        for y in range(min2, max2 + 1):
            label = placeholder.format(maximum - ((y - min2) * interval /
                                                  (rows if rows else 1)))
            result[y - min2][max(offset - len(label), 0)] = label
            result[y - min2][offset - 1] = symbols[0] if y == 0 else symbols[
                1]  # zero tick mark
        # first value is a tick mark across the y-axis
        d0 = series[0][0]
        if _isnum(d0):
            result[rows - scaled(d0)][offset - 1] = symbols[0]
        for i in range(0, len(series)):
            color = colors[i % len(colors)]
            # plot the line
            for x in range(0, len(series[i]) - 1):
                d0 = series[i][x + 0]
                d1 = series[i][x + 1]
                if isnan(d0) and isnan(d1):
                    continue
                if isnan(d0) and _isnum(d1):
                    result[rows - scaled(d1)][x + offset] = colored(
                        symbols[2], color)
                    continue
                if _isnum(d0) and isnan(d1):
                    result[rows - scaled(d0)][x + offset] = colored(
                        symbols[3], color)
                    continue
                y0 = scaled(d0)
                y1 = scaled(d1)
                if y0 == y1:
                    result[rows - y0][x + offset] = colored(symbols[4], color)
                    continue
                result[rows - y1][x + offset] = colored(
                    symbols[5], color) if y0 > y1 else colored(
                        symbols[6], color)
                result[rows - y0][x + offset] = colored(
                    symbols[7], color) if y0 > y1 else colored(
                        symbols[8], color)
                start = min(y0, y1) + 1
                end = max(y0, y1)
                for y in range(start, end):
                    result[rows - y][x + offset] = colored(symbols[9], color)
        return '\n'.join([''.join(row).rstrip() for row in result])

    config = {'height': rows}
    if can_colorize():
        config['colors'] = ['\033[32m']
    print(_plot(data, cfg=config))
