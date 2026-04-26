import requests


def test_memfs_manager_health():
    response = requests.get("http://bears-memfs-manager:8285/health", timeout=5)
    assert response.status_code == 200


def test_den_reachable():
    response = requests.get("http://bears-den:3000/health", timeout=5)
    assert response.status_code == 200


def test_pool_health():
    response = requests.get("http://bears-codepool:3030/health", timeout=5)
    assert response.status_code == 200
