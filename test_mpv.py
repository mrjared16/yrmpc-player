#!/usr/bin/env python3
"""
Simple test script for MPV IPC communication.
Tests basic MPV commands via Unix socket.
"""

import socket
import json
import sys
import time

SOCKET_PATH = "/tmp/rmpc-mpv.sock"

def send_command(sock, command):
    """Send a command to MPV and get response."""
    cmd_json = json.dumps(command) + "\n"
    sock.sendall(cmd_json.encode('utf-8'))
    
    # Read response
    response = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            break
        response += chunk
        if b'\n' in response:
            break
    
    return json.loads(response.decode('utf-8'))

def main():
    print(f"Testing MPV IPC at {SOCKET_PATH}")
    
    try:
        # Connect to MPV socket
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(SOCKET_PATH)
        print("✓ Connected to MPV socket")
        
        # Test 1: Get pause status
        print("\nTest 1: Get pause status")
        response = send_command(sock, {"command": ["get_property", "pause"]})
        print(f"  Response: {response}")
        
        # Test 2: Get volume
        print("\nTest 2: Get volume")
        response = send_command(sock, {"command": ["get_property", "volume"]})
        print(f"  Response: {response}")
        
        # Test 3: Set volume
        print("\nTest 3: Set volume to 50")
        response = send_command(sock, {"command": ["set_property", "volume", 50]})
        print(f"  Response: {response}")
        
        # Test 4: Get time position
        print("\nTest 4: Get time position")
        response = send_command(sock, {"command": ["get_property", "time-pos"]})
        print(f"  Response: {response}")
        
        # Test 5: Get playlist
        print("\nTest 5: Get playlist count")
        response = send_command(sock, {"command": ["get_property", "playlist-count"]})
        print(f"  Response: {response}")
        
        sock.close()
        print("\n✓ All tests passed!")
        return 0
        
    except FileNotFoundError:
        print(f"✗ Error: Socket not found at {SOCKET_PATH}")
        print("  Make sure MPV is running with:")
        print(f"  mpv --idle --no-video --input-ipc-server={SOCKET_PATH}")
        return 1
    except Exception as e:
        print(f"✗ Error: {e}")
        return 1

if __name__ == "__main__":
    sys.exit(main())
