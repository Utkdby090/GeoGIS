GeoGIS DJI pack @VERSION@

GeoGIS reads FLIR thermal photos on its own. DJI thermal photos (R-JPEGs, for
example from a Zenmuse H20T, an M30T or a Mavic 3T) need this pack and DJI's own
Thermal SDK libraries.

1. Unzip this pack straight into the GeoGIS folder, so that geoflux-dirp.exe sits
   next to geogis.exe (in Explorer: Extract All, then pick the GeoGIS folder):
   - setup.exe install: %LOCALAPPDATA%\Programs\GeoGIS
   - .msi install: C:\Program Files\GeoGIS (needs administrator rights)
   - portable zip: the folder you unzipped GeoGIS into
2. Download the DJI Thermal SDK (TSDK) for Windows from DJI's website and accept
   DJI's licence.
3. Copy every file from the SDK's tsdk-core\lib\windows\release_x64 folder
   directly into the dji-sdk folder from this pack. libdirp.dll must be directly
   inside dji-sdk, not in a subfolder.
4. Restart GeoGIS and open a DJI thermal photo from the Inspection pane.

geoflux-dirp.exe is the only program that loads DJI's libraries, so a damaged
photo can stop it but not GeoGIS; GeoGIS starts it again for the next photo.

DJI's licence does not allow us to include their libraries in this pack.
