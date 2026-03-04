// APRS Symbol Icon Generation — Sprite-based with canvas fallbacks for MapLibre
// Sprite sheets from OK-DMR/aprs-symbols (MIT), 64px cells, 16col x 6row per table
// Phase 1: Register canvas fallbacks synchronously (instant render)
// Phase 2: Load sprite PNGs async, upgrade icons seamlessly via map.updateImage()

(function() {
    'use strict';

    var SIZE = 64;
    var HALF = SIZE / 2;
    var CELL = 64;
    var COLS = 16;
    var SPRITE_BASE = 'sprites/aprs-symbols-64-';

    // Color palette for canvas fallbacks
    var C = {
        r: '#ef4444', g: '#10b981', l: '#84cc16', b: '#3b82f6',
        a: '#f59e0b', o: '#f97316', c: '#06b6d4', i: '#6366f1',
        p: '#a855f7', x: '#71717a'
    };

    // Parse compact symbol data: "Name|c;Name|c;..." -> [{name, color}, ...]
    function parseData(str) {
        return str.split(';').map(function(e) {
            var p = e.split('|');
            return { name: p[0], color: C[p[1]] || C.x };
        });
    }

    // Primary table '/' — 94 symbols (ASCII 33 '!' through 126 '~')
    var PRIMARY = parseData(
        'Police|r;Reserved|x;Digi|r;Phone|p;DX Cluster|i;Gateway|i;Small Aircraft|o;' +
        'Mobile Satellite|x;Wheelchair|g;Snowmobile|l;Red Cross|r;Boy Scouts|g;' +
        'House (QTH)|g;X|x;Red Dot|r;Circle 0|a;Circle 1|a;Circle 2|a;Circle 3|a;' +
        'Circle 4|a;Circle 5|a;Circle 6|a;Circle 7|a;Circle 8|a;Circle 9|a;Fire|r;' +
        'Campground|g;Motorcycle|l;Train|x;Car|l;File Server|i;Hurricane|r;' +
        'Aid Station|r;BBS|i;Canoe|c;Reserved|x;Eyeball|g;Farm Vehicle|l;Grid Square|x;' +
        'Hotel|a;TCP/IP|i;Reserved|x;School|a;PC User|i;MacAPRS|i;NTS Station|b;' +
        'Balloon|o;Police|r;TBD|x;Rec Vehicle|l;Space Shuttle|o;SSTV|i;Bus|a;ATV|i;' +
        'NWS Site|b;Helicopter|o;Yacht|c;WinAPRS|i;Jogger|g;Triangle (DF)|a;PBBS|i;' +
        'Large Aircraft|o;Weather Station|b;Dish Antenna|r;Ambulance|r;Bicycle|p;' +
        'Incident Command|r;Fire Dept|r;Horse|l;Fire Truck|r;Glider|o;Hospital|r;' +
        'IOTA|x;Jeep|l;Truck|a;Laptop|i;Mic-E Repeater|r;Node|c;Emergency|r;' +
        'Rover (Dog)|o;Grid Square|x;Antenna|r;Ship|b;Truck Stop|a;18-Wheeler|a;' +
        'Van|l;Water Station|b;xAPRS (Unix)|i;Yagi|r;Shelter|g;Reserved|x;' +
        'TNC Stream Sw1|x;Reserved|x;TNC Stream Sw2|x'
    );

    // Alternate table '\' — 94 symbols (ASCII 33 '!' through 126 '~')
    var ALTERNATE = parseData(
        'Emergency|r;Reserved|x;Star Digi|r;Bank/ATM|a;Reserved|x;Diamond|i;' +
        'Crash Site|r;Cloudy|b;Reserved|x;Snow|b;Church|a;Girl Scouts|g;House HF|g;' +
        'Ambiguous|x;Reserved|x;Circle|a;Reserved|x;Reserved|x;Reserved|x;Reserved|x;' +
        'Reserved|x;Reserved|x;Reserved|x;802.11/WiFi|i;Gas Station|a;Hail|b;Park|g;' +
        'Gale Flag|b;APRStt|i;Car|l;Info Kiosk|i;Hurricane|r;Box|a;Blowing Snow|b;' +
        'Coast Guard|b;Drizzle|b;Smoke|x;Freezing Rain|b;Snow Shower|b;Haze|x;' +
        'Rain Shower|b;Lightning|a;Kenwood|i;Lighthouse|a;Reserved|x;Nav Buoy|c;' +
        'Rocket|o;Parking|a;Earthquake|r;Restaurant|a;Satellite|o;Thunderstorm|b;' +
        'Sunny|a;VORTAC|i;NWS Site|b;Pharmacy|r;Reserved|x;Reserved|x;Wall Cloud|b;' +
        'Reserved|x;Reserved|x;Large Aircraft|o;WX Station|b;Rain|b;ARRL/Field Day|r;' +
        'Blowing Dust|a;Civil Defense|r;DX Spot|i;Sleet|b;Funnel Cloud|r;Gale Flags|b;' +
        'Ham Store|a;Indoor/POI|i;Work Zone|a;SUV/ATV|l;Area|x;Milepost|x;Triangle|a;' +
        'Small Circle|x;Partly Cloudy|b;Reserved|x;Restrooms|a;Ship|b;Tornado|r;' +
        'Truck|a;Van|l;Flooding|b;Wreck|r;Skywarn|b;Shelter|g;Reserved|x;' +
        'TNC Stream Sw1|x;Reserved|x;TNC Stream Sw2|x'
    );

    var TABLES = { '/': PRIMARY, '\\': ALTERNATE };

    // Look up symbol data for (table, code)
    function lookup(table, code) {
        var data = TABLES[table];
        if (!data) return null;
        var offset = code.charCodeAt(0) - 33;
        if (offset < 0 || offset >= data.length) return null;
        return data[offset];
    }

    // Sprite grid position for a symbol code char
    function toGrid(code) {
        var offset = code.charCodeAt(0) - 33;
        if (offset < 0 || offset > 93) return null;
        return { row: Math.floor(offset / COLS), col: offset % COLS };
    }

    // MapLibre image ID for a (table, code) pair
    function imgId(table, code) {
        return 'aprs-' + table + code;
    }

    function createCanvas() {
        var c = document.createElement('canvas');
        c.width = SIZE;
        c.height = SIZE;
        return c;
    }

    // Dark circular backdrop for visibility on any map tile
    function drawBackdrop(ctx) {
        ctx.fillStyle = 'rgba(0, 0, 0, 0.7)';
        ctx.beginPath();
        ctx.arc(HALF, HALF, 27, 0, Math.PI * 2);
        ctx.fill();
    }

    // Canvas fallback: colored circle with ASCII character in white
    function renderFallback(ch, color) {
        var canvas = createCanvas();
        var ctx = canvas.getContext('2d');
        drawBackdrop(ctx);
        // Colored circle
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.arc(HALF, HALF, 18, 0, Math.PI * 2);
        ctx.fill();
        ctx.strokeStyle = 'rgba(255,255,255,0.5)';
        ctx.lineWidth = 2;
        ctx.stroke();
        // Character label
        ctx.fillStyle = '#fff';
        ctx.font = 'bold 22px monospace';
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.fillText(ch, HALF, HALF + 1);
        return ctx.getImageData(0, 0, SIZE, SIZE);
    }

    // === Phase 1: Register canvas fallbacks for ALL valid symbols ===
    function registerFallbacks(map) {
        var tables = ['/', '\\'];
        for (var t = 0; t < tables.length; t++) {
            var table = tables[t];
            for (var c = 33; c <= 126; c++) {
                var ch = String.fromCharCode(c);
                var sym = lookup(table, ch);
                var color = sym ? sym.color : C.x;
                var id = imgId(table, ch);
                if (!map.hasImage(id)) {
                    map.addImage(id, renderFallback(ch, color), { pixelRatio: 1 });
                }
            }
        }
        // Bright default for completely unknown symbols
        if (!map.hasImage('aprs-default')) {
            map.addImage('aprs-default', renderFallback('?', '#22d3ee'), { pixelRatio: 1 });
        }
    }

    // === Phase 2: Load sprite PNGs and seamlessly upgrade icons ===
    function loadSprites(map) {
        var sheets = [
            { table: '/', file: SPRITE_BASE + '0.png' },
            { table: '\\', file: SPRITE_BASE + '1.png' }
        ];
        sheets.forEach(function(entry) {
            var img = new Image();
            img.onload = function() { upgradeFromSprite(map, img, entry.table); };
            img.onerror = function() {
                console.warn('APRS sprite sheet failed to load: ' + entry.file);
            };
            img.src = entry.file;
        });
    }

    function upgradeFromSprite(map, img, table) {
        var canvas = createCanvas();
        var ctx = canvas.getContext('2d');

        for (var c = 33; c <= 126; c++) {
            var ch = String.fromCharCode(c);
            var grid = toGrid(ch);
            if (!grid) continue;

            var id = imgId(table, ch);
            if (!map.hasImage(id)) continue;

            ctx.clearRect(0, 0, SIZE, SIZE);
            drawBackdrop(ctx);
            // Composite sprite cell over dark backdrop
            ctx.drawImage(img,
                grid.col * CELL, grid.row * CELL, CELL, CELL,
                0, 0, SIZE, SIZE);

            map.updateImage(id, ctx.getImageData(0, 0, SIZE, SIZE));
        }
    }

    // === Public API ===

    // Register all icons and start async sprite loading
    window.registerAprsIcons = function(map) {
        registerFallbacks(map);
        loadSprites(map);
    };

    // Symbols with clear, recognizable sprites worth showing on the map.
    // Everything else falls through to the colored circle layer.
    var PROMINENT = {
        '/':  '!#$&\'->.=@CFHKOPRSTUWXY[\\^_`abdefjknorsuvyw<',
        '\\': '!#&>@CKLNOQRSTUW_^ahksuvy'
    };

    // Get MapLibre image ID for a symbol (handles overlay tables)
    // Returns '' for non-prominent symbols so they use the circle layer instead
    window.getSymbolIconId = function(table, code) {
        if (!table || !code) return '';
        var offset = code.charCodeAt(0) - 33;
        if (offset < 0 || offset > 93) return '';
        // Any table char other than '/' maps to alternate table '\'
        var effective = (table === '/') ? '/' : '\\';
        var prom = PROMINENT[effective];
        if (!prom || prom.indexOf(code) < 0) return '';
        return imgId(effective, code);
    };

    // Human-readable symbol name
    window.getSymbolDescription = function(table, code) {
        if (!table || !code) return 'Unknown';
        var effective = (table === '/') ? '/' : '\\';
        var sym = lookup(effective, code);
        return sym ? sym.name : 'Unknown';
    };

    // Symbol color (for badges, UI accents)
    window.getSymbolColor = function(table, code) {
        if (!table || !code) return C.x;
        var effective = (table === '/') ? '/' : '\\';
        var sym = lookup(effective, code);
        return sym ? sym.color : C.x;
    };
})();
