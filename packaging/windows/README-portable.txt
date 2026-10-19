GeoGIS @VERSION@ (portable)

Unzip this folder anywhere and run geogis.exe. No installation or admin rights
are needed. rustgis-cli.exe is the command line tool: run it from a terminal in
this folder, or add the folder to your PATH.

Settings and plugins are kept in your user profile (%APPDATA% and
%LOCALAPPDATA%), not in this folder.

The portable build does not update itself: download a newer zip to upgrade.

DJI thermal photos need the GeoGIS DJI pack: unzip it into this folder, so that
geoflux-dirp.exe sits next to geogis.exe, then add DJI's libraries to the dji-sdk
folder as its README says. FLIR photos work without it.
