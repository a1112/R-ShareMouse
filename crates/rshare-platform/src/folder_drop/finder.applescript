on run coordinates
    set expectedBounds to {}
    repeat with coordinate in coordinates
        set end of expectedBounds to coordinate as integer
    end repeat
    with timeout of 2 seconds
        tell application id "com.apple.finder"
            set matchingFolders to {}
            set windowIds to get id of every Finder window
            repeat with windowId in windowIds
                try
                    set candidate to Finder window id (windowId as integer)
                    if bounds of candidate is expectedBounds then
                        set targetFolder to target of candidate
                        set end of matchingFolders to POSIX path of (targetFolder as alias)
                    end if
                end try
            end repeat
            if (count of matchingFolders) is not 1 then error "Finder window changed, is virtual, or is ambiguous"
            return item 1 of matchingFolders
        end tell
    end timeout
end run
