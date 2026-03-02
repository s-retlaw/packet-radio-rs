// APRS Symbol Icon Generation — Canvas-based icons for MapLibre
// Maps (symbol_table, symbol_code) to rendered 32x32 icons

(function() {
    'use strict';

    // Symbol lookup: [table][code] -> { name, color, shape }
    var SYMBOLS = {
        '/': {
            '-': { name: 'House (QTH)', color: '#10b981', shape: 'house' },
            '>': { name: 'Car', color: '#84cc16', shape: 'car' },
            '_': { name: 'WX Station', color: '#3b82f6', shape: 'wx' },
            'k': { name: 'Truck', color: '#f59e0b', shape: 'truck' },
            'y': { name: 'Yacht', color: '#06b6d4', shape: 'boat' },
            'b': { name: 'Bicycle', color: '#8b5cf6', shape: 'bike' },
            '.': { name: 'X Digi', color: '#ef4444', shape: 'digi' },
            'r': { name: 'Antenna', color: '#ef4444', shape: 'antenna' },
            'O': { name: 'Balloon', color: '#f97316', shape: 'balloon' },
            '$': { name: 'Phone', color: '#a855f7', shape: 'phone' },
            '=': { name: 'Train', color: '#71717a', shape: 'train' },
            'a': { name: 'Ambulance', color: '#ef4444', shape: 'cross' },
            'f': { name: 'Fire', color: '#ef4444', shape: 'fire' },
            'j': { name: 'Jeep', color: '#84cc16', shape: 'car' },
            's': { name: 'Ship', color: '#3b82f6', shape: 'boat' },
            'u': { name: 'Bus', color: '#f59e0b', shape: 'bus' },
            'v': { name: 'Van', color: '#84cc16', shape: 'car' },
            '#': { name: 'Digi', color: '#ef4444', shape: 'digi' },
            '&': { name: 'Gateway', color: '#6366f1', shape: 'gateway' },
            'p': { name: 'Rover (Dog)', color: '#f97316', shape: 'dot' },
            'n': { name: 'Node', color: '#71717a', shape: 'dot' },
            'W': { name: 'NWS Site', color: '#3b82f6', shape: 'wx' },
            'I': { name: 'TCP/IP', color: '#6366f1', shape: 'dot' },
            'K': { name: 'School', color: '#f59e0b', shape: 'dot' },
            'R': { name: 'Rec Vehicle', color: '#84cc16', shape: 'car' },
            'U': { name: 'Bus', color: '#f59e0b', shape: 'bus' },
            'Y': { name: 'Sailboat', color: '#06b6d4', shape: 'boat' },
            '[': { name: 'Runner', color: '#10b981', shape: 'person' },
        },
        '\\': {
            '-': { name: 'House (HF)', color: '#10b981', shape: 'house' },
            '>': { name: 'Car (Alt)', color: '#84cc16', shape: 'car' },
            '_': { name: 'WX Station (Alt)', color: '#3b82f6', shape: 'wx' },
            'O': { name: 'Rocket', color: '#f97316', shape: 'balloon' },
            '#': { name: 'Star Digi', color: '#ef4444', shape: 'digi' },
        }
    };

    var SIZE = 48;
    var HALF = SIZE / 2;

    function createCanvas() {
        var c = document.createElement('canvas');
        c.width = SIZE;
        c.height = SIZE;
        return c;
    }

    // Draw a dark backdrop behind each icon for visibility on any map
    function drawBackdrop(ctx) {
        ctx.fillStyle = 'rgba(0, 0, 0, 0.55)';
        ctx.beginPath();
        ctx.arc(HALF, HALF, 20, 0, Math.PI * 2);
        ctx.fill();
    }

    // Shape drawing functions (48x48 canvas, centered)
    var shapes = {
        house: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.moveTo(HALF, 8);
            ctx.lineTo(40, 22);
            ctx.lineTo(40, 40);
            ctx.lineTo(8, 40);
            ctx.lineTo(8, 22);
            ctx.closePath();
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
        },
        car: function(ctx, color) {
            ctx.fillStyle = color;
            // Car body
            ctx.beginPath();
            ctx.roundRect(6, 16, 36, 20, 4);
            ctx.fill();
            // Roof
            ctx.beginPath();
            ctx.roundRect(12, 8, 24, 14, 3);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.roundRect(6, 16, 36, 20, 4);
            ctx.stroke();
        },
        wx: function(ctx, color) {
            ctx.fillStyle = color;
            // Circle with cross (weather station)
            ctx.beginPath();
            ctx.arc(HALF, HALF, 16, 0, Math.PI * 2);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2.5;
            ctx.stroke();
            // Cross
            ctx.beginPath();
            ctx.moveTo(HALF, 9);
            ctx.lineTo(HALF, 39);
            ctx.moveTo(9, HALF);
            ctx.lineTo(39, HALF);
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
        },
        truck: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.rect(5, 14, 30, 22);
            ctx.fill();
            ctx.beginPath();
            ctx.rect(35, 20, 8, 16);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.rect(5, 14, 38, 22);
            ctx.stroke();
        },
        boat: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.moveTo(9, 30);
            ctx.lineTo(HALF, 42);
            ctx.lineTo(39, 30);
            ctx.lineTo(HALF, 6);
            ctx.closePath();
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
        },
        bike: function(ctx, color) {
            ctx.strokeStyle = color;
            ctx.lineWidth = 3;
            // Two wheels
            ctx.beginPath();
            ctx.arc(15, 32, 9, 0, Math.PI * 2);
            ctx.stroke();
            ctx.beginPath();
            ctx.arc(33, 32, 9, 0, Math.PI * 2);
            ctx.stroke();
            // Frame
            ctx.beginPath();
            ctx.moveTo(15, 32);
            ctx.lineTo(HALF, 16);
            ctx.lineTo(33, 32);
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
        },
        digi: function(ctx, color) {
            ctx.fillStyle = color;
            // Star shape (digipeater)
            ctx.beginPath();
            for (var i = 0; i < 5; i++) {
                var angle = (i * 72 - 90) * Math.PI / 180;
                var x = HALF + 18 * Math.cos(angle);
                var y = HALF + 18 * Math.sin(angle);
                if (i === 0) ctx.moveTo(x, y);
                else ctx.lineTo(x, y);
                angle = ((i * 72 + 36) - 90) * Math.PI / 180;
                x = HALF + 7 * Math.cos(angle);
                y = HALF + 7 * Math.sin(angle);
                ctx.lineTo(x, y);
            }
            ctx.closePath();
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 1.5;
            ctx.stroke();
        },
        antenna: function(ctx, color) {
            ctx.strokeStyle = color;
            ctx.lineWidth = 4;
            // Vertical mast
            ctx.beginPath();
            ctx.moveTo(HALF, 42);
            ctx.lineTo(HALF, 12);
            ctx.stroke();
            // Antenna elements
            ctx.lineWidth = 3;
            ctx.beginPath();
            ctx.moveTo(12, 14);
            ctx.lineTo(36, 14);
            ctx.moveTo(15, 9);
            ctx.lineTo(33, 9);
            ctx.stroke();
            // Base
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(HALF, 42, 4, 0, Math.PI * 2);
            ctx.fill();
        },
        balloon: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(HALF, 19, 15, 0, Math.PI * 2);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
            // String
            ctx.strokeStyle = color;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(HALF, 34);
            ctx.lineTo(HALF, 44);
            ctx.stroke();
        },
        phone: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.roundRect(14, 5, 20, 38, 4);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
            // Screen
            ctx.fillStyle = 'rgba(255,255,255,0.3)';
            ctx.fillRect(17, 11, 14, 22);
        },
        train: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.roundRect(9, 9, 30, 30, 3);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
            // Windows
            ctx.fillStyle = 'rgba(255,255,255,0.4)';
            ctx.fillRect(13, 13, 9, 9);
            ctx.fillRect(26, 13, 9, 9);
        },
        cross: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.moveTo(18, 6);
            ctx.lineTo(30, 6);
            ctx.lineTo(30, 18);
            ctx.lineTo(42, 18);
            ctx.lineTo(42, 30);
            ctx.lineTo(30, 30);
            ctx.lineTo(30, 42);
            ctx.lineTo(18, 42);
            ctx.lineTo(18, 30);
            ctx.lineTo(6, 30);
            ctx.lineTo(6, 18);
            ctx.lineTo(18, 18);
            ctx.closePath();
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 1.5;
            ctx.stroke();
        },
        fire: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.moveTo(HALF, 3);
            ctx.bezierCurveTo(12, 18, 9, 33, HALF, 45);
            ctx.bezierCurveTo(39, 33, 36, 18, HALF, 3);
            ctx.fill();
            // Inner flame
            ctx.fillStyle = '#fcd34d';
            ctx.beginPath();
            ctx.moveTo(HALF, 18);
            ctx.bezierCurveTo(18, 27, 17, 36, HALF, 42);
            ctx.bezierCurveTo(31, 36, 30, 27, HALF, 18);
            ctx.fill();
        },
        bus: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.roundRect(8, 8, 32, 32, 4);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
            // Windows
            ctx.fillStyle = 'rgba(255,255,255,0.4)';
            ctx.fillRect(12, 12, 10, 10);
            ctx.fillRect(26, 12, 10, 10);
        },
        gateway: function(ctx, color) {
            ctx.fillStyle = color;
            // Diamond
            ctx.beginPath();
            ctx.moveTo(HALF, 6);
            ctx.lineTo(42, HALF);
            ctx.lineTo(HALF, 42);
            ctx.lineTo(6, HALF);
            ctx.closePath();
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2;
            ctx.stroke();
            // Arrow in center
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2.5;
            ctx.beginPath();
            ctx.moveTo(18, HALF);
            ctx.lineTo(32, HALF);
            ctx.moveTo(27, HALF - 5);
            ctx.lineTo(32, HALF);
            ctx.lineTo(27, HALF + 5);
            ctx.stroke();
        },
        person: function(ctx, color) {
            ctx.fillStyle = color;
            // Head
            ctx.beginPath();
            ctx.arc(HALF, 12, 7, 0, Math.PI * 2);
            ctx.fill();
            // Body
            ctx.strokeStyle = color;
            ctx.lineWidth = 4;
            ctx.beginPath();
            ctx.moveTo(HALF, 19);
            ctx.lineTo(HALF, 32);
            // Arms
            ctx.moveTo(12, 25);
            ctx.lineTo(36, 25);
            // Legs
            ctx.moveTo(HALF, 32);
            ctx.lineTo(14, 44);
            ctx.moveTo(HALF, 32);
            ctx.lineTo(34, 44);
            ctx.stroke();
        },
        dot: function(ctx, color) {
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(HALF, HALF, 12, 0, Math.PI * 2);
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 2.5;
            ctx.stroke();
        },
    };

    // Default icon for unknown symbols
    function drawDefault(ctx, color) {
        ctx.fillStyle = color || '#71717a';
        ctx.beginPath();
        ctx.arc(HALF, HALF, 12, 0, Math.PI * 2);
        ctx.fill();
        ctx.strokeStyle = 'rgba(255,255,255,0.5)';
        ctx.lineWidth = 2.5;
        ctx.stroke();
    }

    function renderIcon(sym) {
        var canvas = createCanvas();
        var ctx = canvas.getContext('2d');
        drawBackdrop(ctx);
        var drawFn = shapes[sym.shape] || drawDefault;
        drawFn(ctx, sym.color);
        return ctx.getImageData(0, 0, SIZE, SIZE);
    }

    // Register all known APRS icons with a MapLibre map instance
    window.registerAprsIcons = function(map) {
        var tables = Object.keys(SYMBOLS);
        for (var t = 0; t < tables.length; t++) {
            var table = tables[t];
            var codes = Object.keys(SYMBOLS[table]);
            for (var c = 0; c < codes.length; c++) {
                var code = codes[c];
                var sym = SYMBOLS[table][code];
                var id = 'aprs-' + table + code;
                if (!map.hasImage(id)) {
                    map.addImage(id, renderIcon(sym), { pixelRatio: 1 });
                }
            }
        }
    };

    // Get MapLibre image ID for a symbol. Returns empty string if unknown.
    window.getSymbolIconId = function(table, code) {
        if (SYMBOLS[table] && SYMBOLS[table][code]) {
            return 'aprs-' + table + code;
        }
        return '';
    };

    // Get human-readable symbol description
    window.getSymbolDescription = function(table, code) {
        if (SYMBOLS[table] && SYMBOLS[table][code]) {
            return SYMBOLS[table][code].name;
        }
        return 'Unknown';
    };

    // Get symbol color for use in source badges, etc.
    window.getSymbolColor = function(table, code) {
        if (SYMBOLS[table] && SYMBOLS[table][code]) {
            return SYMBOLS[table][code].color;
        }
        return '#71717a';
    };
})();
