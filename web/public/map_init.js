// MapLibre GL JS integration for APRS Viewer
// Global functions called from app.js

var map = null;
var stationClickCallback = null;
var mapClickCallback = null;
var mapReady = false;
var pendingStations = null;
var pendingTracks = null;

// Dark basemap style using Protomaps PMTiles
function darkStyle(tileUrl) {
    return {
        version: 8,
        sources: {
            'protomaps': {
                type: 'vector',
                url: 'pmtiles://' + tileUrl,
                attribution: '&copy; OpenStreetMap'
            }
        },
        glyphs: 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf',
        layers: [
            {
                id: 'background',
                type: 'background',
                paint: { 'background-color': '#09090b' }
            },
            {
                id: 'water',
                type: 'fill',
                source: 'protomaps',
                'source-layer': 'water',
                paint: { 'fill-color': '#0c1323' }
            },
            {
                id: 'land',
                type: 'fill',
                source: 'protomaps',
                'source-layer': 'earth',
                paint: { 'fill-color': '#131316' }
            },
            {
                id: 'roads',
                type: 'line',
                source: 'protomaps',
                'source-layer': 'roads',
                paint: { 'line-color': '#1e1e21', 'line-width': 0.5 }
            },
            {
                id: 'boundaries',
                type: 'line',
                source: 'protomaps',
                'source-layer': 'boundaries',
                paint: { 'line-color': '#2a2a2e', 'line-width': 1, 'line-dasharray': [4, 4] }
            }
        ]
    };
}

// Dark theme paint overrides keyed by liberty layer ID
var darkOverrides = {
    'background':           { 'background-color': '#09090b' },
    'water':                { 'fill-color': '#0c1323' },
    'natural_earth':        '_hide_',
    'park':                 { 'fill-color': '#101012' },
    'park_outline':         { 'line-color': '#1a1a1e' },
    'landuse_residential':  { 'fill-color': '#0e0e10' },
    'landcover_wood':       { 'fill-color': '#0f100f' },
    'landcover_grass':      { 'fill-color': '#0f100f' },
    'landcover_ice':        { 'fill-color': '#101018' },
    'road_motorway':        { 'line-color': '#2e2e32' },
    'road_trunk':           { 'line-color': '#27272a' },
    'road_primary':         { 'line-color': '#27272a' },
    'road_secondary':       { 'line-color': '#222226' },
    'road_minor':           { 'line-color': '#1e1e21' },
    'road_service':         { 'line-color': '#1a1a1e' },
    'road_path':            { 'line-color': '#18181b' },
    'bridge_motorway':      { 'line-color': '#2e2e32' },
    'bridge_trunk':         { 'line-color': '#27272a' },
    'bridge_primary':       { 'line-color': '#27272a' },
    'bridge_secondary':     { 'line-color': '#222226' },
    'bridge_minor':         { 'line-color': '#1e1e21' },
    'tunnel_motorway':      { 'line-color': '#222226' },
    'tunnel_trunk':         { 'line-color': '#1e1e21' },
    'tunnel_primary':       { 'line-color': '#1e1e21' },
    'rail':                 { 'line-color': '#1a1a1e' },
    'rail_dash':            { 'line-color': '#222226' },
    'boundary_country':     { 'line-color': '#2a2a2e' },
    'boundary_state':       { 'line-color': '#222226' },
    'building':             { 'fill-color': '#131316' },
    'building_outline':     { 'line-color': '#1a1a1e' },
    'water_name':           { 'text-color': '#2a3a4a', 'text-halo-color': '#09090b' },
    'waterway_name':        { 'text-color': '#2a3a4a', 'text-halo-color': '#09090b' },
};

// Apply dark overrides directly to a style JSON object (pre-load)
function applyDarkToStyle(style) {
    if (!style || !style.layers) return style;

    for (var i = 0; i < style.layers.length; i++) {
        var layer = style.layers[i];
        var override = darkOverrides[layer.id];

        if (override === '_hide_') {
            if (!layer.layout) layer.layout = {};
            layer.layout.visibility = 'none';
            continue;
        }

        if (override) {
            if (!layer.paint) layer.paint = {};
            for (var prop in override) {
                layer.paint[prop] = override[prop];
            }
            continue;
        }

        // Generic darkening — only override simple string/color values,
        // skip layers with data-driven expressions to avoid null errors
        if (layer.type === 'fill' && layer.paint) {
            var fc = layer.paint['fill-color'];
            if (typeof fc === 'string') {
                layer.paint['fill-color'] = '#101012';
            }
        }
        if (layer.type === 'symbol' && layer.paint) {
            var tc = layer.paint['text-color'];
            if (typeof tc === 'string' || tc === undefined) {
                layer.paint['text-color'] = '#52525b';
                layer.paint['text-halo-color'] = '#09090b';
            }
        }
    }
    return style;
}

// Simple fallback style (no tiles at all)
function simpleStyle() {
    return {
        version: 8,
        sources: {},
        glyphs: 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf',
        layers: [
            {
                id: 'background',
                type: 'background',
                paint: { 'background-color': '#09090b' }
            }
        ]
    };
}

// Add APRS data layers (stations, tracks) to the map
function addAprsLayers() {
    // Add station source (empty initially)
    map.addSource('aprs-stations', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] }
    });

    // Add track source
    map.addSource('aprs-tracks', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] }
    });

    // Track lines layer
    map.addLayer({
        id: 'tracks-line',
        type: 'line',
        source: 'aprs-tracks',
        layout: { visibility: 'none' },
        paint: {
            'line-color': '#4fc3f7',
            'line-width': 2,
            'line-opacity': 0.6,
        }
    });

    // Station circles layer
    map.addLayer({
        id: 'stations-circle',
        type: 'circle',
        source: 'aprs-stations',
        paint: {
            'circle-radius': [
                'match', ['get', 'stationType'],
                'Weather', 7,
                'Mobile', 5,
                6
            ],
            'circle-color': [
                'match', ['get', 'stationType'],
                'Position', '#10b981',
                'Mobile', '#84cc16',
                'Weather', '#3b82f6',
                'Object', '#f59e0b',
                'Item', '#f59e0b',
                'Message', '#a855f7',
                '#71717a'
            ],
            'circle-stroke-width': [
                'case',
                ['get', 'selected'], 3,
                1
            ],
            'circle-stroke-color': [
                'case',
                ['get', 'selected'], '#6366f1',
                'rgba(255,255,255,0.3)'
            ],
        }
    });

    // Station labels
    map.addLayer({
        id: 'stations-label',
        type: 'symbol',
        source: 'aprs-stations',
        layout: {
            'text-field': ['get', 'callsign'],
            'text-font': ['Noto Sans Regular'],
            'text-size': 11,
            'text-offset': [0, 1.5],
            'text-anchor': 'top',
            'text-optional': true,
        },
        paint: {
            'text-color': '#ccc',
            'text-halo-color': '#000',
            'text-halo-width': 1,
        }
    });

    // Click on station
    map.on('click', 'stations-circle', function(e) {
        if (e.features.length > 0 && stationClickCallback) {
            stationClickCallback(e.features[0].properties.callsign);
        }
    });

    // Click on empty map
    map.on('click', function(e) {
        var features = map.queryRenderedFeatures(e.point, { layers: ['stations-circle'] });
        if (features.length === 0 && mapClickCallback) {
            mapClickCallback(e.lngLat.lng, e.lngLat.lat);
        }
    });

    // Cursor changes
    map.on('mouseenter', 'stations-circle', function() {
        map.getCanvas().style.cursor = 'pointer';
    });
    map.on('mouseleave', 'stations-circle', function() {
        map.getCanvas().style.cursor = '';
    });

    // Mark map as ready and apply any buffered data
    mapReady = true;
    if (pendingStations) {
        try { map.getSource('aprs-stations').setData(pendingStations); } catch(e) {}
        pendingStations = null;
    }
    if (pendingTracks) {
        try { map.getSource('aprs-tracks').setData(pendingTracks); } catch(e) {}
        pendingTracks = null;
    }
}

function createMap(containerId, lng, lat, zoom, style) {
    try {
        map = new maplibregl.Map({
            container: containerId,
            style: style,
            center: [lng, lat],
            zoom: zoom,
        });
    } catch (e) {
        console.error('Failed to create map:', e);
        return;
    }

    map.on('error', function(e) {
        console.error('Map error:', e.error || e);
    });

    map.on('load', addAprsLayers);
}

function initMap(containerId, lng, lat, zoom, tileUrl, darkMode) {
    if (typeof maplibregl === 'undefined') {
        console.warn('MapLibre GL JS not loaded, using placeholder');
        return;
    }

    // Register PMTiles protocol if available
    var hasPmtiles = false;
    if (typeof pmtiles !== 'undefined') {
        var protocol = new pmtiles.Protocol();
        maplibregl.addProtocol('pmtiles', protocol.tile);
        hasPmtiles = true;
    }

    // Use PMTiles if local file available
    if (hasPmtiles && tileUrl) {
        createMap(containerId, lng, lat, zoom, darkStyle(tileUrl));
        return;
    }

    // Fetch OpenFreeMap liberty style, apply dark overrides, then create map
    fetch('https://tiles.openfreemap.org/styles/liberty')
        .then(function(resp) {
            if (!resp.ok) throw new Error('Failed to fetch liberty style: ' + resp.status);
            return resp.json();
        })
        .then(function(style) {
            applyDarkToStyle(style);
            createMap(containerId, lng, lat, zoom, style);
        })
        .catch(function(err) {
            console.error('Failed to load liberty style, using fallback:', err);
            createMap(containerId, lng, lat, zoom, simpleStyle());
        });
}

function updateStations(geojsonStr) {
    if (!map) return;
    var data = JSON.parse(geojsonStr);
    if (!mapReady) {
        pendingStations = data;
        return;
    }
    try {
        var source = map.getSource('aprs-stations');
        if (source) {
            source.setData(data);
        }
    } catch (e) {
        console.error('Failed to update stations:', e);
    }
}

function updateTracks(geojsonStr) {
    if (!map) return;
    var data = JSON.parse(geojsonStr);
    if (!mapReady) {
        pendingTracks = data;
        return;
    }
    try {
        var source = map.getSource('aprs-tracks');
        if (source) {
            source.setData(data);
        }
    } catch (e) {
        console.error('Failed to update tracks:', e);
    }
}

function flyTo(lng, lat, zoom) {
    if (map) {
        map.flyTo({ center: [lng, lat], zoom: zoom, duration: 1000 });
    }
}

function onStationClick(callback) {
    stationClickCallback = callback;
}

function onMapClick(callback) {
    mapClickCallback = callback;
}

function destroyMap() {
    if (map) {
        map.remove();
        map = null;
    }
}

function setTracksVisible(visible) {
    if (!map) return;
    var layer = map.getLayer('tracks-line');
    if (layer) {
        map.setLayoutProperty('tracks-line', 'visibility', visible ? 'visible' : 'none');
    }
}

function getMapCenter() {
    if (!map) return "";
    var center = map.getCenter();
    return JSON.stringify({ lng: center.lng, lat: center.lat, zoom: map.getZoom() });
}
