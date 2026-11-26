"""Entry point of the sender engine process.

The backend starts this file with the system Python 3 as a transient systemd
user unit. It only depends on the standard library and the audiobridge package
that sits next to it.
"""

import sys

from audiobridge.engine import main

if __name__ == "__main__":
    sys.exit(main())
